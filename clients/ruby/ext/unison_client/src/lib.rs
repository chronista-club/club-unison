//! Native extension for the `unison-client` Ruby gem.
//!
//! Wraps the Rust `club-unison` crate as Ruby objects. The protocol itself
//! (QUIC transport, channel multiplexing, wire framing) lives in Rust; this
//! layer only bridges Ruby ⇄ Rust values and Ruby's blocking calls ⇄ the
//! async runtime.
//!
//! - `Unison::Client`  — connection lifecycle, wraps `ProtocolClient`
//! - `Unison::Channel` — request/response + event push, wraps `UnisonChannel`
//! - `Unison::Error`   — base class for every failure this binding raises
//!
//! Channel payloads cross the boundary as native Ruby values: `serde_magnus`
//! converts Ruby `Hash`/`Array`/… ⇄ `serde_json::Value`, which the channel's
//! JSON codec consumes.
//!
//! Blocking calls release the GVL while parked on the network (see
//! [`without_gvl`]), so other Ruby threads keep running.

use std::ffi::c_void;
use std::sync::OnceLock;

use magnus::{Error, ExceptionClass, Ruby, Value, function, method, prelude::*};
use serde_json::Value as JsonValue;
use tokio::runtime::Runtime;
use unison::network::ProtocolMessage;
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

/// Runs `f` with Ruby's GVL released, letting other Ruby threads proceed.
///
/// `f` MUST NOT touch any Ruby value or Ruby C API — it executes without the
/// GVL. It runs on the *calling* thread (`rb_thread_call_without_gvl` does not
/// move work elsewhere), so non-`Send` captures are fine.
///
/// A panic inside `f` would otherwise unwind across the `extern "C"`
/// trampoline boundary (= UB / process abort). We [`catch_unwind`] it and
/// surface it as a `Unison::Error` on the calling Ruby thread instead, so a
/// bug in the native layer raises a Ruby exception rather than killing the
/// host process. The exception is built *after* the GVL is reacquired.
///
/// [`catch_unwind`]: std::panic::catch_unwind
///
/// Remaining future refinement: no unblock function is registered, so a
/// blocked call cannot be interrupted by `Thread#kill`. Callers that need a
/// bound can use the explicit-timeout variants (e.g. `Channel#recv_timeout`).
fn without_gvl<F, R>(f: F) -> Result<R, Error>
where
    F: FnOnce() -> R,
{
    /// Carries the closure in and its (panic-checked) result out across the C
    /// boundary.
    struct Payload<F, R> {
        func: Option<F>,
        result: Option<Result<R, String>>,
    }

    unsafe extern "C" fn trampoline<F, R>(arg: *mut c_void) -> *mut c_void
    where
        F: FnOnce() -> R,
    {
        // SAFETY: `arg` is the `&mut Payload` passed below; Ruby invokes this
        // exactly once, on this thread, before `rb_thread_call_without_gvl`
        // returns.
        let payload = unsafe { &mut *(arg as *mut Payload<F, R>) };
        let func = payload
            .func
            .take()
            .expect("without_gvl closure already ran");
        // panic を C 境界の手前で捕まえる。 AssertUnwindSafe: クロージャは &self 等を
        // 借用するが、 panic 後にそれらを再利用しないので unwind safety は破られない。
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(func));
        payload.result = Some(caught.map_err(panic_message));
        std::ptr::null_mut()
    }

    let mut payload = Payload::<F, R> {
        func: Some(f),
        result: None,
    };
    // SAFETY: `trampoline::<F, R>` interprets the data pointer as
    // `&mut Payload<F, R>`, which is exactly what we hand it; no unblock
    // function is used.
    unsafe {
        rb_sys::rb_thread_call_without_gvl(
            Some(trampoline::<F, R>),
            (&raw mut payload).cast(),
            None,
            std::ptr::null_mut(),
        );
    }
    // ここでは GVL を再取得済みなので `unison_error` (= Ruby API) を呼んでよい。
    match payload.result.expect("without_gvl closure did not run") {
        Ok(value) => Ok(value),
        Err(msg) => Err(unison_error(format!("panic in native Unison call: {msg}"))),
    }
}

/// Extracts a human-readable message from a panic payload.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Current `Ruby` handle.
///
/// Safe inside any function bound into Ruby: such code only runs while a Ruby
/// method is on the stack, so the handle is always available.
fn ruby() -> Ruby {
    Ruby::get().expect("Unison binding used outside a Ruby thread")
}

/// Builds a `Unison::Error` exception carrying `msg`.
///
/// `Unison::Error` is defined once in `init`; this re-fetches it via the
/// (idempotent) module handle rather than caching — error construction is
/// never a hot path.
fn unison_error(msg: impl Into<String>) -> Error {
    let class: ExceptionClass = ruby()
        .define_module("Unison")
        .and_then(|m| m.const_get("Error"))
        .expect("Unison::Error class is not defined");
    Error::new(class, msg.into())
}

/// Turns a `NetworkError` into a `Unison::Error`.
fn net_err(e: NetworkError) -> Error {
    unison_error(e.to_string())
}

/// Serializes an inbound `ProtocolMessage` into the Ruby `Hash` shape returned
/// by `Channel#recv` / `#recv_timeout` (`"id"` / `"type"` / `"method"` /
/// `"payload"`).
fn msg_to_ruby(msg: &ProtocolMessage) -> Result<Value, Error> {
    let payload = msg.payload_as_value().map_err(net_err)?;
    let out = serde_json::json!({
        "id": msg.id,
        "type": msg.msg_type,
        "method": msg.method,
        "payload": payload,
    });
    serde_magnus::serialize(&ruby(), &out)
}

/// The Unison protocol generation this client is built against.
fn protocol_target() -> &'static str {
    "1.0.0"
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
            .map_err(|e| unison_error(format!("Unison::Client.new failed: {e}")))?;
        Ok(Self { inner })
    }

    /// `client.connect(url)` — opens the QUIC connection to `url`.
    ///
    /// Blocks the calling thread until the handshake completes (the GVL is
    /// released, so other Ruby threads keep running). Raises `Unison::Error`
    /// on failure.
    fn connect(&self, url: String) -> Result<(), Error> {
        without_gvl(|| runtime().block_on(self.inner.connect(&url)))?.map_err(net_err)
    }

    /// `client.connected?` — whether the QUIC connection is currently open.
    fn connected(&self) -> Result<bool, Error> {
        without_gvl(|| runtime().block_on(self.inner.is_connected()))
    }

    /// `client.disconnect` — closes the QUIC connection.
    ///
    /// Raises `Unison::Error` only if the close itself errors.
    fn disconnect(&self) -> Result<(), Error> {
        without_gvl(|| runtime().block_on(self.inner.disconnect()))?.map_err(net_err)
    }

    /// `client.open_channel(name)` — opens a named channel, returning a
    /// `Unison::Channel`. Raises `Unison::Error` on failure.
    fn open_channel(&self, name: String) -> Result<Channel, Error> {
        let inner =
            without_gvl(|| runtime().block_on(self.inner.open_channel(&name)))?.map_err(net_err)?;
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
    /// value. Raises `Unison::Error` on a protocol error or timeout.
    fn request(&self, method: String, payload: Value) -> Result<Value, Error> {
        let ruby = ruby();
        // Ruby → Rust conversion needs the GVL; do it before releasing it.
        let req: JsonValue = serde_magnus::deserialize(&ruby, payload)?;
        let resp: JsonValue = without_gvl(|| {
            runtime().block_on(self.inner.request::<JsonValue, JsonValue>(&method, &req))
        })?
        .map_err(net_err)?;
        serde_magnus::serialize(&ruby, &resp)
    }

    /// `channel.send_event(method, payload)` — sends a fire-and-forget event
    /// (no response is awaited).
    fn send_event(&self, method: String, payload: Value) -> Result<(), Error> {
        let event: JsonValue = serde_magnus::deserialize(&ruby(), payload)?;
        without_gvl(|| runtime().block_on(self.inner.send_event(&method, &event)))?.map_err(net_err)
    }

    /// `channel.recv` — blocks until the next inbound event (server push or
    /// other non-response message), returned as a Ruby `Hash` with keys
    /// `"id"`, `"type"`, `"method"`, `"payload"`.
    ///
    /// The GVL is released while waiting, so other Ruby threads run. This form
    /// blocks indefinitely; use [`Self::recv_timeout`] for a bounded wait.
    /// (Interruption via `Thread#kill` is a remaining future refinement.)
    fn recv(&self) -> Result<Value, Error> {
        let msg = without_gvl(|| runtime().block_on(self.inner.recv()))?.map_err(net_err)?;
        msg_to_ruby(&msg)
    }

    /// `channel.recv_timeout(seconds)` — like [`Self::recv`] but raises
    /// `Unison::Error` if no message arrives within `seconds`.
    ///
    /// 無期限 block を避けたい caller 向けの bounded 版。 timeout は
    /// `tokio::time::timeout` で実装し、 GVL は同様に解放する。
    fn recv_timeout(&self, seconds: f64) -> Result<Value, Error> {
        let dur = std::time::Duration::from_secs_f64(seconds);
        let msg = without_gvl(|| {
            runtime().block_on(async { tokio::time::timeout(dur, self.inner.recv()).await })
        })?
        .map_err(|_elapsed| unison_error(format!("recv timed out after {seconds}s")))?
        .map_err(net_err)?;
        msg_to_ruby(&msg)
    }

    /// `channel.close` — closes the channel and stops its receive loop.
    fn close(&self) -> Result<(), Error> {
        without_gvl(|| runtime().block_on(self.inner.close()))?.map_err(net_err)
    }
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Unison")?;
    module.define_module_function("protocol_target", function!(protocol_target, 0))?;

    // `Unison::Error` — base class for every failure raised by this binding.
    module.define_error("Error", ruby.exception_standard_error())?;

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
    channel.define_method("recv_timeout", method!(Channel::recv_timeout, 1))?;
    channel.define_method("close", method!(Channel::close, 0))?;

    Ok(())
}
