# 移行ガイド — v2.0 までの破壊的変更

既存の caller が v2.0 へ上がる際に把握すべき破壊的変更をまとめる。
内容は [`CHANGELOG.md`](../CHANGELOG.md) を一次情報とする。

破壊的変更は大きく 4 つ:

0. v2.0 の呼び出し元の無い public API の削除（Rust API のみ、wire は不変）
1. lib 名の rename（`club_unison` → `unison`、v1.0）
2. v0.9.0 の wire format pivot（rkyv → buffa）
3. RPC 廃止 → Unified Channel（v0.2.0、過去変更だが現行 API の前提）

---

## 0. v1.9 → v2.0 — 呼び出し元の無い public API の削除

2.0.0 は、workspace と利用側の repo を grep して呼び出し元が無いと確かめた public API を
削除した major。**wire bytes は変わらない**ので、TS / Swift / Ruby client との互換は
そのままで、client 側の変更は要らない。破壊的なのは Rust の API surface だけで、
いずれも機械的に置き換えられる。

### 置き換え

| 1.9 | 2.0 |
|---|---|
| `server.listen(addr)` / `spawn_listen` / `spawn_listen_shared` / `spawn_listen_with_cert` / `spawn_listen_shared_with_cert` | `Arc::new(server).listener(addr).spawn()`（background、`ServerHandle` を返す）。呼び出し側で待つなら `.run()`。cert を載せるなら `.cert(src)` を挟む |
| `ProtocolClient::new_default()` | `ProtocolClient::insecure_localhost()`（中身は同じ。証明書を検証しないことを名前で示す） |
| `QuicClient::new()` / `QuicServer::new(server)` | `QuicClient::insecure_localhost()` / `QuicServer::builder(server).build()` |
| `NetworkError::Protocol("channel closed")` の文字列一致 | `NetworkError::ChannelEof(_)` の match、または `err.is_normal_close()` |
| `ConnectionEvent::Connected { remote_addr, context }` / `Disconnected { remote_addr }` の分解 | どちらも `connection_id` が増えたので `..` を足す |
| `unison::network::quic::{UnisonStream, TypedFrame}` | `unison::network::{UnisonStream, TypedFrame}` |
| `unison::network::quic::{read_typed_frame, write_typed_frame, CHANNEL_ACK_METHOD, FRAME_TYPE_*}` | `unison::network::frame::*` |
| `use unison::prelude::*`（`prelude` は廃止） | 使う型を明示して import（`unison::{ProtocolClient, ProtocolServer, UnisonChannel, NetworkError, ...}`、`unison::parser::{SchemaParser, ...}`） |
| `ProtocolServer::spawn_listen_webtransport(...)` | `WebTransportServer::new(server, cert).bind(addr)` → `start_with_shutdown` |

`SchemaParser::parse` のエラー型、`UnisonPacket` の builder、`PacketType` の変換、
証明書生成ヘルパー、`unison-agent` crate など、残りの項目は CHANGELOG の
「Migration (1.9 → 2.0)」の表を参照。

```rust
// 旧（1.9）
server.listen("[::1]:8080").await?;
let client = ProtocolClient::new_default()?;

// 新（2.0）
use std::sync::Arc;
Arc::new(server).listener("[::1]:8080").run().await?; // background なら .spawn() で ServerHandle
let client = ProtocolClient::insecure_localhost()?;
```

channel の正常終端は文字列ではなく variant で判定する:

```rust
match channel.recv().await {
    Ok(msg) => { /* … */ }
    Err(e) if e.is_normal_close() => { /* 相手が channel を閉じた（ChannelEof） */ }
    Err(e) => return Err(e),
}
```

---

## 1. lib 名の rename — `club_unison` → `unison`

**v1.0 時点の破壊的変更（source-breaking）**。chronista-club 命名規則に合わせ、
公開 crate の lib 名を bare name に戻した。

### 影響

- `crates/unison-protocol/Cargo.toml` の `[lib].name`: `club_unison` → `unison`
- 全ソースの `use` 行が変わる

```rust
// 旧
use club_unison::ProtocolServer;
use club_unison::network::UnisonChannel;

// 新（v1.0）
use unison::ProtocolServer;
use unison::network::UnisonChannel;
```

### 不変なもの

- **crates.io の package 名は `club-unison` のまま**。`Cargo.toml` の dep 行
  （`club-unison = "..."`）は変更不要。
- 変わるのは code path（`use` 行）だけ。

> 補足: lib 名は v0.5.0 で `unison`、v0.6.0 で `club_unison`（full rename
> policy）、v1.0 で再び `unison` に戻った。`club-` prefix は crates.io の
> global namespace 衝突回避が責務であり、code path には不要、という整理。

---

## 2. wire format pivot — rkyv → buffa（v0.9.0）

v0.8.x までは wire format に **rkyv 0.7 archive** を使っていた。v0.9.0 で
**buffa（Anthropic 製 Protocol Buffers）** に乗り換えた。

### wire レイアウトの変化

```text
# 旧（v0.8.x）
[rkyv-encoded UnisonPacketHeader (56 bytes fixed)] [rkyv-encoded payload]

# 新（v0.9.0+）
[u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes (zstd 圧縮の場合あり)]
```

### 接続互換性

**v0.8.x ↔ v0.9.0+ の binary は互換性なし**。v0.8.x のクライアント・サーバとは
接続できない。**双方を v0.9.0 以降に揃える**こと。

### API の変化

- `UnisonPacket<T: Payloadable>`（generic）→ `UnisonPacket`（非ジェネリック）。
  caller が任意の codec で encode した `Vec<u8>` を渡す形に簡素化。
- `crate::packet::Payloadable` trait と `RkyvPayload` 等の payload 型を全削除。
- `ProtocolMessage` / `MessageType` の PascalCase enum・直 field access の
  caller API は**維持**（wire の内部実装のみ変更）。

### pivot の理由

- **polyglot 親和性**: rkyv は Rust 固有、buffa は protobuf wire format で
  多言語 SDK 化が容易（TS SDK の前提）
- **schema evolution**: protobuf の field number 互換性で前方/後方互換が取れる

---

## 3. RPC 廃止 → Unified Channel（v0.2.0）

v0.2.0 で **RPC を全廃し、全通信を channel に統一**した。現行 API の前提なので、
古い RPC スタイルのコードベースから上がる場合は把握しておく。

### 削除された API

- `register_handler()` / `call()` / `open_typed_channel()` — 旧 RPC メソッド
- `ProtocolClientTrait` / `ProtocolServerTrait` 等の旧トレイト
- KDL の `service` / `method` / `send` / `recv` 構文（書いてもエラーにならず無視される。
  schema-lint も通るので、残っていても気づきにくい）

### 置き換え

| 旧（RPC） | 新（Unified Channel） |
|---|---|
| `server.register_handler(...)` | `server.register_channel(name, handler)` |
| `client.call(...)` | `client.open_channel(name)` → `UnisonChannel` |
| KDL `service` / `method` | KDL `channel` / `request` + `returns` / `event` |

```kdl
// 旧（廃止。書いてもエラーにならず無視される）
service "MyService" {
    method "doThing" { ... }
}

// 新
channel "my_channel" from="client" lifetime="persistent" {
    request "DoThing" {
        field "key" type="string"
        returns "Done" { field "ok" type="bool" }
    }
}
```

`MessageType` も 10 variant から 4（`Request` / `Response` / `Event` /
`Error`）に簡素化された。

---

## 4. その他の注目すべき変更

### TLS API（v0.7.0 / v0.8.0）

build.rs での証明書自動生成・埋め込みが廃止された。trust / cert は operator が
明示する:

```rust
// v0.8.0+ Builder API（推奨）
let client = QuicClient::builder()
    .trust_anchors(TrustAnchors::System)
    .build()?;
let server = QuicServer::builder(Arc::new(server))
    .cert_source(CertSource::dev_localhost())
    .build();
```

`QuicServer::configure_server()` / `QuicClient::configure_client()` の
引数なし版は v0.9.0 で削除済み。`*_with(...)` 明示版か Builder API を使う。

### datagram channel の追加（v0.10.0、additive）

KDL channel に `backend="datagram" channel_id=N` 属性が追加された。既存の
stream channel スキーマ・caller コードは**無改修**（`backend` 省略は
`"stream"` 解釈、純粋 additive）。datagram を使う場合のみ schema に属性追加
+ codegen 再実行 + `register_channel_datagram` / `open_datagram_channel`。

### MSRV

v0.9.0 で Rust 1.95 に bump。

---

## 5. v1.x deferred（v1.0 時点の正直な未対応）

以下は v1.0 ではまだ対応していない:

- **proto-descriptor codegen** — `ProtoCodec` は使えるが、KDL → proto
  descriptor の自動生成は未対応
- **Node native WebTransport** — Node では polyfill が必要
- **Safari / Firefox の WebTransport** — Chromium 系のみ公式サポート
- **per-channel codec override** — connection-level codec 一律のみ
- **auto-reconnect helper** — library は auto-reconnect しない（caller 責務）

---

**最終更新**: 2026-11-04（v2.2.1）
