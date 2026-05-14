//! よく使用される型と関数のprelude
//!
//! このモジュールは、Unison Protocolで頻繁に使用される型やトレイトを
//! 一括でインポートできるようにします。
//!
//! # 使用例
//!
//! ```rust
//! use club_unison::prelude::*;
//! ```

// パーサー関連
pub use crate::parser::{
    Channel, ChannelEvent, ChannelFrom, ChannelLifetime, ChannelMessage, ChannelRequest,
    ParsedSchema, SchemaParser,
};

// Codec 関連
pub use crate::codec::{Codec, CodecError, Decodable, Encodable, JsonCodec, ProtoCodec};

// コードジェネレータ関連
pub use crate::codegen::{CodeGenerator, RustGenerator, TypeScriptGenerator};

// ネットワーク関連
pub use crate::network::{
    ConnectionEvent, ConnectionEventReceiver, ProtocolClient, ProtocolServer, ServerHandle,
    UnisonChannel,
};

// エラー型
pub use crate::network::NetworkError as UnisonNetworkError;
pub use crate::parser::ParseError as UnisonParseError;

// メインエントリポイント
pub use crate::UnisonProtocol;
