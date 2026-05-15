//! Wire format abstraction (v0.9.0 で導入、 v0.10+ 拡張用 hook)
//!
//! v0.9.0 から default wire format は [`crate::packet`] module 経由の
//! buffa (Anthropic 製 protobuf) encoding (= variable-size header + length-prefix
//! boundary)。 v0.8 系で使われていた rkyv 56-byte fixed header は廃止された
//! (= breaking change、 詳細は `CHANGELOG.md` と `spec/02 §8.4`)。
//!
//! v0.10+ では本 module の [`WireFormat`] trait を base に、 **MessagePack**
//! / **CBOR** 等の追加 pluggable 実装を並列に追加する想定。 現時点では
//! trait 表明のみで、 既存 packet 経路 (= buffa direct) は変更しない。
//!
//! # Future direction
//!
//! - `MessagePackWire` — `zerompk` 等経由の MessagePack wire
//!   (polyglot + コンパクト)
//! - `CborWire` — `ciborium` 経由の CBOR wire (IETF 標準互換)
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
    /// 例: `"buffa"`, `"msgpack"`, `"cbor"`。
    fn name() -> &'static str;
}
