# UnisonPacket — バイナリパケット層

**最終更新**: 2026-11-04
**ステータス**: Stable (v0.9.0 buffa pivot 以降の wire、 v2.0 で未使用 field を整理)

`UnisonPacket` は stream 経路 (= `UnisonChannel` の request / response / event) で
`ProtocolMessage` を運ぶ wire-level frame。 datagram 経路 (= `DatagramChannel`) は
本 packet を **経由しない** (`[varint channel_id][payload]` のみ、 [datagram-channel.md](./datagram-channel.md))。

wire layout 全体と buffa 採用理由は [wire-format.md](./wire-format.md) を参照。 本書は
packet 層の Rust API と header の意味論に絞る。

## 1. Layout

```text
[u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes (may be zstd compressed)]
```

## 2. Header

[`proto/protocol.proto`](../crates/unison-protocol/proto/protocol.proto) の `PacketHeader` と
[`packet/header.rs`](../crates/unison-protocol/src/packet/header.rs) の `UnisonPacketHeader` が 1:1。

| field | 型 | 意味 |
|---|---|---|
| `version` | u8 | protocol version。 現在 `0x01`。 不一致は `IncompatibleVersion` |
| `packet_type` | u8 | `PacketType::{Data=0x00, Control=0x01}`。 未知値は `packet_type()` が `Err(raw)` |
| `flags` | u16 | `PacketFlags`。 現在 bit 0 `COMPRESSED` のみ |
| `payload_length` | u32 | 圧縮前 payload 長 |
| `compressed_length` | u32 | 圧縮後 payload 長。 0 = 非圧縮 |
| `timestamp` | u64 | Unix ns |

proto の field 6 / 8-11 (旧 `sequence_number` / `stream_id` / `message_id` / `response_to` /
`correlation_id`) は送信側が常に default で wire に乗っていなかったため v2.0 で削除し
`reserved` にした。 順序保証・request/response 対応・keep-alive は QUIC stream と
`ProtocolMessage.id` が担うので header 側には持たない。 リアルタイム配信で seq /
timestamp が要る場合は datagram payload 側に置く (= header ではなく経路が違う)。

`from_proto` は u32 → u8/u16 の縮小で **飽和** させる (truncate しない)。 version=257 が
1 に化けて互換 gate を素通りする、 という untrusted 入力の穴を塞ぐため。

## 3. Rust API

```rust
use unison::packet::{PacketType, UnisonPacket, UnisonPacketHeader};

// 既定 (= PacketType::Data、 default PacketConfig)
let packet = UnisonPacket::new(payload_bytes)?;

// header を明示
let header = UnisonPacketHeader::new(PacketType::Control);
let packet = UnisonPacket::with_header(header, payload_bytes)?;

let bytes = packet.to_bytes();                 // 送信
let restored = UnisonPacket::from_bytes(&bytes)?; // 受信 (version gate + size gate)
let header = restored.header()?;               // header のみ parse
let payload = restored.payload()?;             // 必要なら解凍して返す
```

production の呼び出し元は 2 方向で形が違う:

- **送信**: `network::ProtocolMessage::into_frame` が buffa encode した `ProtocolMessage`
  を `UnisonPacket::new` で包む
- **受信**: typed frame から取り出した bytes に対し、 caller が `UnisonPacket::from_bytes`
  を直接呼んでから `ProtocolMessage::from_frame` に渡す (= `network/stream.rs` の
  `UnisonStream`、 `network/client.rs` の `read_channel_ack`、 `network/dispatch.rs` の
  `client_accept_bi_loop` / `handle_incoming_stream`)

## 4. 圧縮

`PacketConfig` (= [`packet/config.rs`](../crates/unison-protocol/src/packet/config.rs)):

- `compression.threshold` 2048 byte 以上で zstd を試し、 縮んだ時だけ採用して
  `COMPRESSED` を立てる
- `compression.level` 既定 1 (最速)
- `max_payload_size` 既定 16 MiB。 両方向で bound するが、 測る対象が違う:
  - serialize: packet 全体 (= `4 + header + payload`、 圧縮時は圧縮後 payload) を測る
  - deserialize: 圧縮時は header の `payload_length` (= 申告された展開後サイズ)、
    非圧縮時は payload 長を測る。 `UnisonPacket::from_bytes` は受け取った bytes 全体も
    同じ上限で弾く
- 圧縮 payload は `zstd::bulk::decompress` に申告サイズを **出力上限** として渡して展開し、
  超えた時点で打ち切る (= `DecompressionFailed`)。 全量を展開してから測ると検査の前に
  任意量のメモリを確保させられるため (= 圧縮爆弾対策)。 申告より短く終わった場合は
  `DecompressedSizeMismatch`

stream 経路では packet の外側の typed frame に上限 8 MiB (= `network/frame.rs` の
`MAX_MESSAGE_SIZE`) があるので、 受信できる圧縮後 packet の大きさは実質そちらで決まる
(= 16 MiB は展開後サイズの上限として効く)。

`CompressionConfig::disabled()` で圧縮を止められる (test / 既圧縮 payload 用)。

## 5. エラー

`SerializationError` (= [`packet/serialization.rs`](../crates/unison-protocol/src/packet/serialization.rs)):
`CompressionFailed` / `DecompressionFailed` / `PacketTooLarge` / `DecompressedSizeMismatch` /
`InvalidHeader` / `HeaderLengthOutOfRange` / `IncompatibleVersion` / `SerializationFailed` /
`DeserializationFailed`。 `NetworkError::FrameSerialization` で network 層へ持ち上がる。

## 6. テスト

- unit: `cargo test -p club-unison packet`
- 他言語との byte 一致: `tests/fixtures/wire/*.hex` は commit 済みの golden。
  `tests/test_small_wire_byte_compat.rs` は Rust の encoder 出力をこの golden と **比較**
  する (= 生成はしない。 意図して wire を変えたときだけ `UPDATE_WIRE_FIXTURES=1` で
  再生成する)。 TS (`clients/typescript/tests/wire/byte_compat.test.ts`) は同じ golden を
  直接読み、 Swift (`clients/swift/Tests/UnisonClientTests/WireByteCompatTests.swift`) は
  test resource (`Fixtures/wire/`) にコピーした golden と突き合わせる

## 関連

- [wire-format.md](./wire-format.md) — layout と buffa 採用理由
- [datagram-channel.md](./datagram-channel.md) — packet を経由しない datagram 経路
- [spec/02](../spec/02-unified-channel/SPEC.md) §8.4
