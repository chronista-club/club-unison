English | [日本語](https://github.com/chronista-club/club-unison/blob/main/README.ja.md)

# Unison Protocol

A type-safe QUIC communication framework, defined by KDL schema.

[![Crates.io](https://img.shields.io/crates/v/club-unison.svg)](https://crates.io/crates/club-unison)
[![Build Status](https://github.com/chronista-club/club-unison/actions/workflows/ci.yml/badge.svg)](https://github.com/chronista-club/club-unison/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/chronista-club/club-unison/blob/main/LICENSE)

```toml
[dependencies]
# crates.io package = `club-unison`, Rust crate identifier = `unison`
club-unison = "2.4.1"
tokio = { version = "1.52", features = ["full"] }
```

```rust
use unison::network::{CertSource, TrustAnchors};
use unison::network::quic::{QuicClient, QuicServer};

// Server: pick the TLS cert source explicitly via the `CertSource` enum.
let server_config = QuicServer::configure_server_with(CertSource::dev_localhost()).await?;

// Client: pick the trust anchor explicitly via the `TrustAnchors` enum.
let client_config = QuicClient::configure_client_with(TrustAnchors::System).await?;

// Internal mesh (fixed pair): one keypair for both ends.
use unison::network::InternalMeshKeypair;
let pair = InternalMeshKeypair::generate(["broker.local".into(), "*.unison.local".into()])?;
// pair.server_cert_source → server side / pair.client_trust_anchors → client side

// Internal mesh (many servers): a private CA issuing per-server certs.
use unison::network::MeshCa;
let ca = MeshCa::generate()?;
let server_cert = ca.issue(["cp.internal".into()])?; // server-side CertSource
let client_trust = ca.trust_anchors();               // shared by every client (one CA to trust)
```

See the [CHANGELOG](https://github.com/chronista-club/club-unison/blob/main/CHANGELOG.md) entries for v0.7.0 (TLS introduced), v0.9.0 (deprecated `configure_*()` removed), and v1.0.0-rc.3 (`MeshCa` private CA) for the trust model design in depth.

---

## What happens when you connect

The flow from connect, to opening a channel, to actually exchanging data:

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server

    C->>S: QUIC connect (TLS 1.3 — automatic)
    S->>C: Identity (server name, available channels)
    C->>S: open_channel("users")
    Note over C,S: QUIC bidirectional stream established
    C->>S: request("CreateUser", {name, email})
    S->>C: response({id, created_at})
    S->>C: event("UserCreated", {id})
```

1. The client opens a QUIC connection. TLS 1.3 certificates are auto-generated for dev.
2. The server replies with its Identity — name, version, and the list of available channels.
3. When the client opens a channel, a QUIC bidirectional stream is established for it.
4. From there, request/response, event push, and raw bytes all flow freely over that one channel.

**Everything is a channel.** There is no separate "RPC" concept — it's unified under `UnisonChannel`.

---

## Writing a server

```rust
use std::sync::Arc;
use unison::ProtocolServer;
use unison::network::UnisonChannel;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = ProtocolServer::with_identity(
        "my-server", "1.0.0", "com.example.myservice",
    );

    // Register a channel handler.
    server.register_channel("users", |_ctx, stream| async move {
        let channel = UnisonChannel::new(stream);
        loop {
            match channel.recv().await {
                Ok(msg) => {
                    channel.send_response(msg.id, &msg.method, &json!({"id": "1"})).await?;
                }
                Err(_) => break,
            }
        }
        Ok(())
    }).await;

    // Spawn in the background with graceful-shutdown support.
    let handle = Arc::new(server).listener("[::1]:8080").spawn().await?;
    println!("server up at {}", handle.local_addr());

    // Serve until Ctrl-C, then shut down gracefully.
    tokio::signal::ctrl_c().await?;
    handle.shutdown().await?;
    Ok(())
}
```

## Writing a client

```rust
use unison::ProtocolClient;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Dev only: skips server certificate verification (loopback targets only).
    let client = ProtocolClient::insecure_localhost()?;
    client.connect("[::1]:8080").await?;

    // Open a channel and do a request/response.
    let users = client.open_channel("users").await?;
    let response: serde_json::Value = users.request("CreateUser", &json!({
        "name": "Alice",
        "email": "alice@example.com"
    })).await?;
    println!("user created: {}", response);

    users.close().await?;

    Ok(())
}
```

---

## UnisonChannel

A single channel supports three communication patterns side-by-side:

```
Client                              Server
  |                                    |
  |-- open_channel("users") ---------> |  (QUIC bidi stream)
  |                                    |
  |<-- UnisonChannel ----------------->|  UnisonChannel
  |    .request()    -> Protocol frame  |    .recv()
  |    .send_event() -> Protocol frame  |    .send_response()
  |    .send_raw()   -> Raw frame      |    .send_raw()
  |    .recv()       <- Protocol frame  |    .recv_raw()
  |    .recv_raw()   <- Raw frame      |
```

```rust
// Request / response.
let res: serde_json::Value = channel.request("CreateUser", &payload).await?;
channel.send_response(id, method, &payload).await?;

// Event push (one-way).
channel.send_event("UserCreated", &payload).await?;

// Raw bytes — bypasses buffa/zstd. Useful for audio, etc.
channel.send_raw(&pcm_data).await?;
let data = channel.recv_raw().await?;
```

A single byte at the head of each frame distinguishes Protocol frames (`0x00`, buffa + zstd) from Raw frames (`0x01`, raw bytes). Payloads larger than 2 KB are automatically compressed with zstd.

---

## Connection events

```rust
use unison::ConnectionEvent;

let mut events = server.subscribe_connection_events();
tokio::spawn(async move {
    while let Ok(event) = events.recv().await {
        match event {
            ConnectionEvent::Connected { remote_addr, .. } => {
                println!("connected: {}", remote_addr);
            }
            ConnectionEvent::Disconnected { remote_addr, .. } => {
                println!("disconnected: {}", remote_addr);
            }
        }
    }
});
```

## KDL schema

Define a protocol in KDL and the Rust / TypeScript code is generated for you:

```kdl
protocol "my-service" version="1.0.0" {
    namespace "com.example.myservice"

    channel "users" from="client" lifetime="persistent" {
        request "CreateUser" {
            field "name" type="string" required=#true
            field "email" type="string" required=#true

            returns "UserCreated" {
                field "id" type="string" required=#true
                field "created_at" type="timestamp" required=#true
            }
        }
    }
}
```

---

## State Sync (`unison::sync`)

Keep an in-memory object store (`Origin` ↔ `Slave`) automatically synchronized over QUIC — including multi-hop `Relay` nodes (`Origin → Relay(Slave+Origin) → Slave`):

- **2-channel QUIC transport**:
  - `unison.sync` (client → server): `Join` handshake (`SyncSnapshot`) and Slave `PushBatch` (`SyncBatch`).
  - `unison.sync.feed` (server → client, server-initiated stream): continuous `SyncFrame` broadcast feed from `Origin` to all joined Slaves.
- **Canonical Protobuf wire + zero-allocation reads**: payloads are stored as `bytes::Bytes` and encoded with [`buffa`](https://crates.io/crates/buffa) (`proto/sync.proto`). Callers can inspect slot payloads in place with **0 heap allocations** via `buffa::MessageView<'a>` (`with_slot` / `get_owned_view` / `decode_view`).
- **Convergence guarantees** (formally verified with Quint): monotone `version`, `origin_seq` ordering, idempotent `apply`, `dirty` protection while local edits are in flight, and automatic periodic `Flush` + `KeepAlive` tasks.

```rust
use std::sync::Arc;
use bytes::Bytes;
use unison::{ProtocolClient, ProtocolServer};
use unison::sync::{ConnectedSlave, Origin, Relay, Slave, SyncConfig, SyncServer, decode_view};
use unison::sync::proto::SyncObjectView;

// 1. Attach SyncServer (Origin) to a ProtocolServer
let origin = Arc::new(Origin::new());
origin.insert(1, Bytes::from_static(b"initial-state"), 100);
let sync_server = Arc::new(SyncServer::new(origin.clone(), SyncConfig::default()));

let server = Arc::new(ProtocolServer::with_identity("sync-node", "2.4.0", "club.chronista.sync"));
sync_server.attach(&server).await;

// 2. Connect a Slave over QUIC — initial Snapshot + background Flush / KeepAlive / Feed loops run automatically
let client = Arc::new(ProtocolClient::insecure_localhost()?);
client.connect("[::1]:8080").await?;
let connected = ConnectedSlave::connect(client, Arc::new(Slave::new()), SyncConfig::default()).await?;

// 3. Zero-allocation read from a slot via buffa::MessageView<'a>
let state: Option<Bytes> = connected.slave().with_slot(1, |obj| obj.state.clone());

// Or borrow a zero-copy MessageView<'a> directly from a protobuf-encoded payload buffer:
// let view: SyncObjectView<'_> = decode_view(&buf)?;
```

See [spec/05-sync/SPEC.md](https://github.com/chronista-club/club-unison/blob/main/spec/05-sync/SPEC.md) and [design/sync.md](https://github.com/chronista-club/club-unison/blob/main/design/sync.md) for the protocol specification and architecture.

---

## Workspace crates

| Crate | Description |
|-------|-------------|
| [`unison-protocol`](https://github.com/chronista-club/club-unison/tree/main/crates/unison-protocol) | Core library. Published on crates.io as `club-unison`; the Rust crate identifier is `unison`. KDL schema, QUIC, channels, packets, `unison::sync`. |
| [`unison-mcp`](https://github.com/chronista-club/club-unison/tree/main/crates/unison-mcp) | MCP bridge (`unison-mcp` binary). Discovers a server's KDL at runtime and synthesizes one MCP tool per `request`, so AI agents can call Unison servers. |
| [`unison-cli`](https://github.com/chronista-club/club-unison/tree/main/crates/unison-cli) | Developer CLI (`unison` binary): `ping`, `call`, `sniff`, `mock` (stub server from a KDL file), `schema-lint`, `schema-diff`, `schema-check`. |

## Polyglot clients

The server stays Rust; clients are first-class siblings under [`clients/`](https://github.com/chronista-club/club-unison/tree/main/clients). Wire-format conformance and protocol semantics are owned by this framework, so each client interops byte-for-byte with the Rust server.

| Client | Transport | Distribution |
|--------|-----------|--------------|
| [`clients/typescript`](https://github.com/chronista-club/club-unison/tree/main/clients/typescript) | WebTransport (HTTP/3) | npm `@chronista-club/unison-client` |
| [`clients/ruby`](https://github.com/chronista-club/club-unison/tree/main/clients/ruby) | raw QUIC (FFI → Rust) | RubyGems `unison-client` |
| [`clients/swift`](https://github.com/chronista-club/club-unison/tree/main/clients/swift) | raw QUIC (Apple `NWProtocolQUIC`, ALPN `"unison"`) | SPM (`Package.swift` at repo root) — see note |

> **Swift package consumption**: `Package.swift` lives at the **repo root** (because SPM's versioned remote dependency `.package(url:from:)` requires a root manifest). The implementation lives under `clients/swift/` and is referenced via `path:` from the root manifest. Releases track the monorepo git tag (`vX.Y.Z` = Rust workspace version).
>
> ```swift
> .package(url: "https://github.com/chronista-club/club-unison.git", from: "2.4.1")
> // target: .product(name: "UnisonClient", package: "club-unison")
> ```
>
> For local side-by-side development, a path dependency also works: `.package(path: "../club-unison")`.

---

## Development

```bash
git clone https://github.com/chronista-club/club-unison
cd club-unison
cargo build

# Tests (lib unit + integration tests)
RUSTFLAGS="-C symbol-mangling-version=v0" cargo test --workspace
```

**IPv4 / IPv6 both work.** `[::1]:port` (IPv6 loopback), `127.0.0.1:port` (IPv4 loopback), and hostnames (resolved via DNS) all connect. The client picks a local bind address that matches the target's address family. CLI defaults and most examples lean IPv6; if you pass only a bare port (e.g. `8080`) it falls back to `[::1]`.

## Documentation

- [Core concept](https://github.com/chronista-club/club-unison/blob/main/spec/01-core-concept/SPEC.md) — Everything is a Channel
- [Unified Channel Protocol](https://github.com/chronista-club/club-unison/blob/main/spec/02-unified-channel/SPEC.md) — KDL schema, channel routing, code generation
- [Discovery](https://github.com/chronista-club/club-unison/blob/main/spec/04-discovery/SPEC.md) — Runtime KDL schema discovery (`unison.discovery`)
- [State Sync](https://github.com/chronista-club/club-unison/blob/main/spec/05-sync/SPEC.md) & [Sync Design](https://github.com/chronista-club/club-unison/blob/main/design/sync.md) — `unison::sync` Origin ↔ Slave state synchronization & zero-allocation `buffa::MessageView` reads
- [Channel guide](https://github.com/chronista-club/club-unison/blob/main/guides/channel-guide.md) — practical guide
- [Specs](https://github.com/chronista-club/club-unison/tree/main/spec) / [Design docs](https://github.com/chronista-club/club-unison/tree/main/design) / [Guides](https://github.com/chronista-club/club-unison/tree/main/guides) — full documentation index

## License

MIT License — [LICENSE](https://github.com/chronista-club/club-unison/blob/main/LICENSE)

---

> **Inspired by**: [Unison](https://www.youtube.com/watch?v=_VIeV_LZXHM) by Captain Houshou Marine ⚓
> The name also carries the meaning of being "on the same waves" — exactly what this framework is for.
