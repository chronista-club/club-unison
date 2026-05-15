# design/wire-format.md — Wire Format Pluggable 設計

**バージョン**: 0.1 (v0.9.0 で導入)
**最終更新**: 2026-05-15
**ステータス**: Draft (= trait 表明のみ、 具体実装は v0.10+)

---

## 1. 背景

Unison Protocol は仕様 ([spec/02](../spec/02-unified-channel/SPEC.md) §2.1) で
**多言語サポート** を core principle に掲げている (Rust / TypeScript / Python /
Go 等)。 一方で v0.8.x までの wire format は **rkyv 0.7 archive** に固定されて
おり、 Rust 内 hot path では zero-copy で最速だが polyglot 通信では補助 layer
が必要だった。

v0.9.0 から **wire format を pluggable** にする方向で再設計する。 ただし v0.9.0
時点では **trait 表明と拡張余地の確保** のみとし、 具体実装の追加は v0.10+ に
分割する (= ミニマム原則)。

---

## 2. 要件

| # | 要件 | v0.9.0 | v0.10+ |
|---|------|--------|--------|
| 1 | wire format の trait 抽象化 | ✅ `WireFormat` trait 導入 | — |
| 2 | 既存 caller の breaking ゼロ | ✅ `crate::packet` 経路は不変 | — |
| 3 | rkyv の thin wrapper (`RkyvWire`) | ❌ deferred | ✅ |
| 4 | buffa (Anthropic 製 protobuf) wire | ❌ deferred | ✅ |
| 5 | MessagePack wire (zerompk 等) | ❌ deferred | ✅ |
| 6 | channel negotiation で format 選択 | ❌ deferred | ✅ |
| 7 | spec/02 への wire format 言及 | ✅ § 8.4 追加 | 拡張 |

---

## 3. 設計

### 3.1 WireFormat trait

```rust
pub trait WireFormat {
    type EncodeError: Error + Send + Sync + 'static;
    type DecodeError: Error + Send + Sync + 'static;

    fn name() -> &'static str;
}
```

minimal、 method なし (= encode/decode signature は v0.10 で具体化)。 v0.9.0
では「**こういう抽象が将来入る**」 という表明のみ。

### 3.2 v0.10 で追加予定の 3 実装

| 実装 | format | 採用候補 crate | 主用途 |
|------|--------|---------------|--------|
| `RkyvWire` | rkyv archive | `rkyv 0.7` (現状) → 0.8 評価 | Rust 内 zero-copy hot path |
| `BuffaWire` | Protocol Buffers | `buffa 0.5` (Anthropic) | polyglot, schema evolution |
| `MessagePackWire` | MessagePack | `zerompk` 等 | polyglot, コンパクト |

### 3.3 Channel 単位 / Connection 単位 の format 選択

v0.10+ で議論。 候補:
- 接続初期 handshake で client / server がサポート format を交換、 共通最大集
  合から選ぶ
- channel 定義 (KDL schema) に `wire_format = "buffa"` を直書き
- ProtocolMessage の payload 内で format を mark

---

## 4. なぜ v0.9.0 で実装まで進めないか

### 4.1 ミニマム原則

「user 0」 段階で full pluggable 実装は over-engineering。 trait 抽象化は
**意図表明 + 拡張 hook** として価値あり、 具体実装は **needs (= polyglot client
の登場や パフォーマンス要求)** が表面化してから。

### 4.2 wire format breaking は spec level

3 format 間で **packet 互換は取れない** (= 各 format で binary 異なる)。 channel
negotiation 込みで spec/02 update が必要、 これは別 release sprint で実施する
方が議論が clean。

### 4.3 強結合 reality

現状 `network/mod.rs:86` の `ProtocolMessage` が rkyv direct 使用、 packet と
network が tightly coupled。 wire pluggable 化は **ProtocolMessage の format
非依存化** も伴うため、 packet/* + network/* 両方の redesign が必要。 v0.9.0
scope を超える。

---

## 5. v0.10 への引き継ぎ

- [ ] `RkyvWire` 実装 + 既存 packet path の wrap
- [ ] `BuffaWire` 実装 (= test_proto_buffa.rs を production path へ昇格)
- [ ] `MessagePackWire` 実装 (zerompk vs rmp-serde の評価込み)
- [ ] `ProtocolMessage` を format 非依存に redesign
- [ ] channel negotiation の spec / KDL schema 拡張
- [ ] migration guide (rkyv-only → pluggable)

詳細は creo-memories `unison` Atlas の `wire-format-pluggable` 系 memory を参照。

---

## 6. 関連

- [spec/02-unified-channel/SPEC.md](../spec/02-unified-channel/SPEC.md) §8.4
- [src/wire/mod.rs](../crates/unison-protocol/src/wire/mod.rs) — WireFormat trait
- [README (buffa)](https://crates.io/crates/buffa) — Anthropic 製 protobuf
- [README (zerompk)](https://crates.io/crates/zerompk) — MessagePack zero-alloc
