//! Wire format abstraction (v0.9.0 で導入、 v0.10+ 拡張用 hook)
//!
//! v0.9.0 時点の default wire format は [`crate::packet`] module 経由の
//! rkyv archive (zero-copy)。 v0.10+ では本 module の [`WireFormat`] trait
//! を base に **buffa (Anthropic 製 protobuf)** や **MessagePack** 等の
//! pluggable 実装を追加していく予定。
//!
//! 現時点の本 module は **拡張準備のための trait 表明** のみで、 既存の
//! [`crate::packet::Payloadable`] 経路 (= rkyv 直結) は変更しない。 caller
//! 視点では breaking なし。
//!
//! # Future direction
//!
//! - `BuffaWire` — Anthropic 製 [`buffa`](https://crates.io/crates/buffa)
//!   経由の Protocol Buffers wire (polyglot 通信向け)
//! - `MessagePackWire` — `zerompk` 等経由の MessagePack wire
//!   (polyglot + コンパクト)
//! - `RkyvWire` — 既存 [`crate::packet::Payloadable`] を本 trait に
//!   adapt する thin wrapper (= migration path)
//!
//! 詳細は `design/wire-format.md` 参照。

use std::error::Error;

/// 任意の wire format を抽象化する trait。
///
/// 実装側は encode / decode と format identifier (= negotiation 用) を
/// 提供する。 v0.9.0 では default 実装を提供せず、 既存 packet 経路を
/// 引き続き利用する。 v0.10+ で具体実装が並ぶ。
pub trait WireFormat {
    /// encode 失敗時の error 型
    type EncodeError: Error + Send + Sync + 'static;

    /// decode 失敗時の error 型
    type DecodeError: Error + Send + Sync + 'static;

    /// この wire format の識別子。
    ///
    /// log / channel negotiation / debug 表示で使用する固定文字列。
    /// 例: `"rkyv"`, `"buffa"`, `"msgpack"`。
    fn name() -> &'static str;
}
