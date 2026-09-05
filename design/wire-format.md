# design/wire-format.md — Wire Format 設計 (v0.9.0 buffa pivot)

**バージョン**: 1.0 (v0.9.0 で buffa pivot 完了)
**最終更新**: 2026-09-05
**ステータス**: Stable (= buffa wire 確定)

---

## 1. 背景

Unison Protocol は仕様 ([spec/02](../spec/02-unified-channel/SPEC.md) §2.1) で
**多言語サポート** を core principle に掲げている (Rust / TypeScript / Python /
Go 等)。 一方で v0.8.x までの wire format は **rkyv 0.7 archive** に固定されて
おり、 Rust 内 hot path では zero-copy で最速だが polyglot 通信では補助 layer
が必要だった。

v0.9.0 で **wire format を buffa (Anthropic 製 Protocol Buffers) に乗り換え**、 rkyv
依存を完全削除した。 他 format (= MessagePack / CBOR) の pluggable 化は必要になった
時点で設計する (= v0.9.0 で置いた空の `WireFormat` trait は実装が付かないまま
v2.0 で削除)。

### なぜ buffa か

- **polyglot 親和性**: protobuf wire format は多言語 SDK 化が容易、 KDL schema-first
  哲学とも整合
- **schema evolution**: protobuf の field number 互換性で前方/後方互換が取れる
- **Anthropic ecosystem alignment**: Anthropic 製 protobuf、 club-unison が Claude /
  Anthropic 周辺 tool との接続を取りやすい
- **zero-copy view 対応**: buffa-codegen が `*View` 型を生成、 必要に応じて zero-copy
  read も可能 (= rkyv に劣後しない hot path option)

---

## 2. v0.9.0 wire format

### 2.1 Packet layout

```text
[u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes (may be zstd compressed)]
```

- 先頭 4 byte: header bytes 長 (big-endian u32)
- header 部: [`proto::PacketHeader`](../crates/unison-protocol/proto/protocol.proto)
  を buffa でエンコードした可変長
- payload 部: caller が任意の codec (JsonCodec / ProtoCodec / raw) で encode した bytes、
  圧縮は header の `compressed_length > 0` + `flags::COMPRESSED` で表現

旧 v0.8 系の rkyv 56-byte fixed header は v0.9.0 で **完全削除** された。

### 2.2 PacketHeader / ProtocolMessage schema

[`crates/unison-protocol/proto/protocol.proto`](../crates/unison-protocol/proto/protocol.proto)
で proto3 として定義:

- `ProtocolMessage` (id / method / msg_type / payload)
- `MessageType` enum (REQUEST / RESPONSE / EVENT / ERROR)
- `PacketHeader` (version / packet_type / flags / payload_length / compressed_length /
  timestamp。 field 6 / 8-11 は v2.0 で `reserved`、 詳細は [packet.md](./packet.md) §2)

build.rs が compile し `$OUT_DIR/protocol.{mod,rs,__view.rs,...}` に出力、
`crate::proto` で expose される。

---

## 3. 設計上の判断 record

### 3.1 v0.9.0 で rkyv 削除まで踏み込んだ理由

当初の v0.9.0 plan では **trait 表明 + rkyv default 維持** のミニマム approach も
候補だった。 ただし以下の理由で **rkyv 完全削除 + buffa 全面切替** に踏み切った:

- **rkyv 0.7 → 0.8 移行で trait bound 地獄に遭遇**: 既存 packet 構造を保ったまま
  rkyv を上げると lifetime / trait bound 関係が大きく変わり、 redesign 不可避
- **どうせ redesign するなら buffa pivot で済ませる**: 二回 wire format breaking
  change を打つより、 一度の v0.9.0 sweep で完了させる方が ecosystem 影響少
- **polyglot road map が明確化**: creo-memories / nostos / 他 club-* project が
  polyglot 通信を要求し始めている、 rkyv に縛られる理由が薄い

### 3.2 互換性なし (= clean breaking change)

rkyv ↔ buffa 間で **packet 互換は取れない** (= binary が全く異なる)。 v0.8.x ←→
v0.9.0 間で wire を喋らせるには明示的な migration が必要 (= v0.8.x client → v0.9.0
server は接続できない、 逆も同様)。 v0.9.0 を major bump 扱いとし、 published 前
段階 (= crates.io 上は v0.8.2 まで) なので migration guide は不要と判断。

---

## 4. 今後

### 4.1 wire format 系
- [ ] 他 format (MessagePack / CBOR) が実際に要求された時点で trait 抽象と
  channel negotiation (= `wire_format="..."` 等) を設計する。 先行して抽象だけ
  置くことはしない

### 4.2 datagram backend (= v0.10.0 で channel API 統合決定)

v0.9.0 の `QuicClient::send_datagram` / `recv_datagram` (= connection-level MVP) は
v0.10.0 で **channel API narrative に統合** された。 詳細設計は
[`design/datagram-channel.md`](./datagram-channel.md) を参照。

v0.10.0 で確定した骨子:

- 1 channel = 1 (virtual) stream のメンタルモデル維持、 datagram channel は `channel_id`
  で識別される virtual stream
- KDL `channel "X" backend="datagram" channel_id=N` で宣言、 1 channel = 1 backend strict
- Wire format: `[varint channel_id] [buffa-encoded event payload]`
- 型 API: `DatagramChannel<C>` を `UnisonChannel<C>` と別型分離
- v0.9.0 connection-level API は escape hatch として残存

v0.10+ で残存 task:

- [ ] Mixed backend channel (= 同 KDL channel に stream / datagram event 共存) の許容化判断
- [ ] datagram channel の bench 拡充 (= channel API 経由の demux overhead 計測)

---

## 5. 関連

- [spec/02-unified-channel/SPEC.md](../spec/02-unified-channel/SPEC.md) §8.4 (= wire format)、 §8.5 (= datagram channel)
- [design/datagram-channel.md](./datagram-channel.md) — datagram channel 設計 living doc (v0.10.0)
- [crates/unison-protocol/proto/protocol.proto](../crates/unison-protocol/proto/protocol.proto) — wire schema
- [design/packet.md](./packet.md) — packet 層の API と header 意味論
- [README (buffa)](https://crates.io/crates/buffa) — Anthropic 製 protobuf
