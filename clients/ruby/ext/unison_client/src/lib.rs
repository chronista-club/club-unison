//! Native extension for the `unison-client` Ruby gem.
//!
//! Wraps the Rust `club-unison` crate as Ruby objects. The protocol itself
//! (QUIC transport, channel multiplexing, wire framing) lives in Rust; this
//! layer only bridges Ruby ⇄ Rust values and Ruby's blocking calls ⇄ the
//! async runtime.
//!
//! - `Unison::Client`  — connection lifecycle, wraps `ProtocolClient`
//! - `Unison::Channel` — request/response + event push, wraps `UnisonChannel`
//!
//! Channel payloads cross the boundary as native Ruby values: `serde_magnus`
//! converts Ruby `Hash`/`Array`/… ⇄ `serde_json::Value`, which the channel's
//! JSON codec consumes.

use std::sync::OnceLock;

use magnus::{Error, Ruby, Value, function, method, prelude::*};
use serde_json::Value as JsonValue;
use tokio::runtime::Runtime;
use unison::{NetworkError, ProtocolClient, UnisonChannel};

/// Process-wide multi-thread tokio runtime backing every blocking bridge.
///
/// One long-lived runtime: the QUIC reactor should outlive individual calls,
/// and building a runtime per call would be wasteful. Created lazily on first
/// use.
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("failed to build the Unison tokio runtime"))
}

/// Current `Ruby` handle.
///
/// Safe inside any function bound into Ruby: such code only runs while a Ruby
/// method is on the stack, so the handle is always available.
fn ruby() -> Ruby {
    Ruby::get().expect("Unison binding used outside a Ruby thread")
}

/// Builds a Ruby `RuntimeError` carrying `msg`.
///
/// A dedicated `Unison::Error` hierarchy is a deliberate later refinement —
/// for now every failure surfaces as `RuntimeError`.
fn runtime_error(msg: impl Into<String>) -> Error {
    Error::new(ruby().exception_runtime_error(), msg.into())
}

/// Turns a `NetworkError` into a Ruby `RuntimeError`.
fn net_err(e: NetworkError) -> Error {
    runtime_error(e.to_string())
}

/// The Unison protocol generation this client is built against.
fn protocol_target() -> &'static str {
    "1.0.0-rc.1"
}

/// `Unison::Client` — a QUIC-backed Unison protocol client.
///
/// `ProtocolClient`'s methods all take `&self`, so no interior mutability is
/// needed: the wrapped value is shared read-only and the QUIC state is
/// managed internally by the Rust crate.
#[magnus::wrap(class = "Unison::Client", free_immediately, size)]
struct Client {
    inner: ProtocolClient,
}

impl Client {
    /// `Unison::Client.new` — builds a default QUIC-backed client.
    ///
    /// Does not open a connection; call `#connect` for that.
    ///
    /// **Warning**: backed by `ProtocolClient::new_default()`, which builds an
    /// **insecure** client — TLS certificate verification is skipped (intended
    /// for loopback / development). A secure constructor taking explicit trust
    /// anchors is future work.
    fn new() -> Result<Self, Error> {
        let inner = ProtocolClient::new_default()
            .map_err(|e| runtime_error(format!("Unison::Client.new failed: {e}")))?;
        Ok(Self { inner })
    }

    /// `client.connect(url)` — opens the QUIC connection to `url`.
    ///
    /// Blocks the calling thread (and, for now, the Ruby VM) until the
    /// handshake completes. Raises `RuntimeError` on failure.
    fn connect(&self, url: String) -> Result<(), Error> {
        runtime().block_on(self.inner.connect(&url)).map_err(net_err)
    }

    /// `client.connected?` — whether the QUIC connection is currently open.
    fn connected(&self) -> bool {
        runtime().block_on(self.inner.is_connected())
    }

    /// `client.disconnect` — closes the QUIC connection.
    ///
    /// Raises `RuntimeError` only if the close itself errors.
    fn disconnect(&self) -> Result<(), Error> {
        runtime().block_on(self.inner.disconnect()).map_err(net_err)
    }

    /// `client.open_channel(name)` — opens a named channel, returning a
    /// `Unison::Channel`. Raises `RuntimeError` on failure.
    fn open_channel(&self, name: String) -> Result<Channel, Error> {
        let inner = runtime()
            .block_on(self.inner.open_channel(&name))
            .map_err(net_err)?;
        Ok(Channel { inner })
    }
}

/// `Unison::Channel` — a request/response + event-push channel.
///
/// Constructed only via `Unison::Client#open_channel`; it has no public
/// allocator. Payloads are native Ruby values (`Hash`/`Array`/scalars),
/// carried over the channel's JSON codec.
#[magnus::wrap(class = "Unison::Channel", free_immediately, size)]
struct Channel {
    inner: UnisonChannel,
}

impl Channel {
    /// `channel.request(method, payload)` — sends a request and blocks until
    /// the matching response arrives. Returns the response payload as a Ruby
    /// value. Raises `RuntimeError` on a protocol error or timeout.
    fn request(&self, method: String, payload: Value) -> Result<Value, Error> {
        let ruby = ruby();
        let req: JsonValue = serde_magnus::deserialize(&ruby, payload)?;
        let resp: JsonValue = runtime()
            .block_on(self.inner.request::<JsonValue, JsonValue>(&method, &req))
            .map_err(net_err)?;
        serde_magnus::serialize(&ruby, &resp)
    }

    /// `channel.send_event(method, payload)` — sends a fire-and-forget event
    /// (no response is awaited).
    fn send_event(&self, method: String, payload: Value) -> Result<(), Error> {
        let event: JsonValue = serde_magnus::deserialize(&ruby(), payload)?;
        runtime()
            .block_on(self.inner.send_event(&method, &event))
            .map_err(net_err)
    }

    /// `channel.recv` — blocks until the next inbound event (server push or
    /// other non-response message), returned as a Ruby `Hash` with keys
    /// `"id"`, `"type"`, `"method"`, `"payload"`.
    ///
    /// Note: blocks the Ruby VM until a message arrives — GVL release and a
    /// timeout variant are future refinements.
    fn recv(&self) -> Result<Value, Error> {
        let msg = runtime().block_on(self.inner.recv()).map_err(net_err)?;
        let payload = msg.payload_as_value().map_err(net_err)?;
        let out = serde_json::json!({
            "id": msg.id,
            "type": msg.msg_type,
            "method": msg.method,
            "payload": payload,
        });
        serde_magnus::serialize(&ruby(), &out)
    }

    /// `channel.close` — closes the channel and stops its receive loop.
    fn close(&self) -> Result<(), Error> {
        runtime().block_on(self.inner.close()).map_err(net_err)
    }
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Unison")?;
    module.define_module_function("protocol_target", function!(protocol_target, 0))?;

    let client = module.define_class("Client", ruby.class_object())?;
    client.define_singleton_method("new", function!(Client::new, 0))?;
    client.define_method("connect", method!(Client::connect, 1))?;
    client.define_method("connected?", method!(Client::connected, 0))?;
    client.define_method("disconnect", method!(Client::disconnect, 0))?;
    client.define_method("open_channel", method!(Client::open_channel, 1))?;

    let channel = module.define_class("Channel", ruby.class_object())?;
    channel.define_method("request", method!(Channel::request, 2))?;
    channel.define_method("send_event", method!(Channel::send_event, 2))?;
    channel.define_method("recv", method!(Channel::recv, 0))?;
    channel.define_method("close", method!(Channel::close, 0))?;

    Ok(())
}
