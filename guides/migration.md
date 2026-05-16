# 移行ガイド — v1.0 までの破壊的変更

既存の caller が v1.0 へ上がる際に把握すべき破壊的変更をまとめる。
内容は [`CHANGELOG.md`](../CHANGELOG.md) を一次情報とする。

破壊的変更は大きく 3 つ:

1. lib 名の rename（`club_unison` → `unison`）
2. v0.9.0 の wire format pivot（rkyv → buffa）
3. RPC 廃止 → Unified Channel（v0.2.0、過去変更だが現行 API の前提）

---

## 1. lib 名の rename — `club_unison` → `unison`

**最新の破壊的変更（source-breaking）**。chronista-club 命名規則に合わせ、
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
- KDL の `service` / `method` / `send` / `recv` 構文

### 置き換え

| 旧（RPC） | 新（Unified Channel） |
|---|---|
| `server.register_handler(...)` | `server.register_channel(name, handler)` |
| `client.call(...)` | `client.open_channel(name)` → `UnisonChannel` |
| KDL `service` / `method` | KDL `channel` / `request` + `returns` / `event` |

```kdl
// 旧（非推奨、削除済み）
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
    .build();
let server = QuicServer::builder(server)
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

- **TypeScript codegen の datagram 対応** — Rust codegen のみ datagram 対応、
  TS generator は v1.x
- **proto-descriptor codegen** — `ProtoCodec` は使えるが、KDL → proto
  descriptor の自動生成は未対応
- **Node native WebTransport** — Node では polyfill が必要
- **Safari / Firefox の WebTransport** — Chromium 系のみ公式サポート
- **per-channel codec override** — connection-level codec 一律のみ
- **auto-reconnect helper** — library は auto-reconnect しない（caller 責務）

---

**最終更新**: 2026-05-17（v1.0）
