//! # Unison Protocol
//!
//! **Unison Protocol** は、KDLベースの型安全な通信フレームワークで、
//! 複数言語向けの自動コード生成によるシームレスな分散型ノード間通信を実現します。
//!
//! ## 機能
//!
//! - **型安全な通信**: KDLプロトコル定義からの自動コード生成
//! - **多言語サポート**: Rust、TypeScriptなど複数言語向けのノード実装コード生成
//! - **QUICプロトコル**: 高速で信頼性の高い双方向ストリーム通信
//! - **スキーマ検証**: コンパイル時およびランタイムでのプロトコル検証
//! - **非同期ファースト**: async/awaitサポートを基盤から組み込み
//!
//! ## クイックスタート
//!
//! ```rust,no_run
//! # use anyhow::Result;
//! # #[tokio::main]
//! # async fn main() -> Result<()> {
//! use club_unison::{UnisonProtocol, NetworkError};
//!
//! // プロトコルスキーマを読み込み
//! let mut protocol = UnisonProtocol::new();
//! // protocol.load_schema(include_str!("../schemas/ping_pong.kdl"))?;
//!
//! // サーバーを作成し、チャネルハンドラーを登録
//! let server = protocol.create_server();
//! // server.register_channel("ping", |ctx, stream| async { Ok(()) }).await;
//! // server.listen("[::1]:8080").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## コア概念
//!
//! - **Protocol**: チャネル、メッセージ、型を定義するトップレベルコンテナ
//! - **Channel**: Request/Response + Event push をサポートする双方向通信路
//! - **Message**: 型付きフィールドを持つ構造化データ型
//!
//! ## コード生成
//!
//! `UnisonProtocol::generate_rust_code()` / `generate_typescript_code()` で
//! KDLスキーマから型付きコードを生成できます。

pub mod codec;
pub mod codegen;
pub mod network;
pub mod parser;

// プロトコル定義のコアモジュール
pub mod core;

// フレーム層モジュール
pub mod packet;

// CGPベースのコンテキストモジュール
pub mod context;

// Wire format pluggable hook (v0.9.0 で導入、 v0.10+ で具体実装拡張)
pub mod wire;

// buffa-generated protocol types (v0.9.0 buffa pivot)
//
// build.rs が `proto/protocol.proto` を compile し `$OUT_DIR/protocol.{mod,rs,__view.rs,...}` を出力。
// `.mod.rs` を include すると main types + zero-copy `*View` + `__buffa::{ext,oneof,view}` まで一括 expose。
// caller は `crate::proto::{ProtocolMessage, MessageType, PacketHeader, ProtocolMessageView, PacketHeaderView}` で access 可能。
//
// 注: buffa-codegen 出力の main `.rs` には `#[allow(clippy::*)]` が無いので、 module
// 全体で抑制する (= sub-module の `__buffa` には codegen 側で allow が付いている)。
#[allow(clippy::derivable_impls)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/protocol.mod.rs"));
}

// よく使用される型と関数のprelude
pub mod prelude;

// preludeの型を内部で使用
use codegen::{CodeGenerator, RustGenerator, TypeScriptGenerator};
use parser::{ParseError as UnisonParseError, ParsedSchema, SchemaParser};

// よく使用されるトレイトとクライアント/サーバーの再エクスポート
pub use network::{
    ConnectionEvent, ConnectionEventReceiver, NetworkError, ProtocolClient, ProtocolServer,
    ServerHandle, UnisonChannel,
};

/// Unison Protocolのメインエントリポイント
pub struct UnisonProtocol {
    schemas: Vec<ParsedSchema>,
    parser: SchemaParser,
}

impl UnisonProtocol {
    /// 新しいUnison Protocolインスタンスを作成
    pub fn new() -> Self {
        Self {
            schemas: Vec::new(),
            parser: SchemaParser::new(),
        }
    }

    /// KDL文字列からプロトコルスキーマを読み込み
    pub fn load_schema(&mut self, schema: &str) -> Result<(), UnisonParseError> {
        let parsed = self.parser.parse(schema)?;
        self.schemas.push(parsed);
        Ok(())
    }

    /// 読み込んだスキーマからRustコードを生成
    pub fn generate_rust_code(&self) -> Result<String, Box<dyn std::error::Error>> {
        let generator = RustGenerator::new();
        let type_registry = crate::parser::TypeRegistry::new(); // 一時的な空のレジストリ
        let mut code = String::new();

        for schema in &self.schemas {
            code.push_str(&generator.generate(schema, &type_registry)?);
            code.push('\n');
        }

        Ok(code)
    }

    /// 読み込んだスキーマからTypeScriptコードを生成
    pub fn generate_typescript_code(&self) -> Result<String, Box<dyn std::error::Error>> {
        let generator = TypeScriptGenerator::new();
        let type_registry = crate::parser::TypeRegistry::new(); // 一時的な空のレジストリ
        let mut code = String::new();

        for schema in &self.schemas {
            code.push_str(&generator.generate(schema, &type_registry)?);
            code.push('\n');
        }

        Ok(code)
    }

    /// 新しいUnisonクライアントを作成
    pub fn create_client(&self) -> Result<ProtocolClient, anyhow::Error> {
        ProtocolClient::new_default()
    }

    /// 新しいUnisonサーバーを作成
    pub fn create_server(&self) -> ProtocolServer {
        ProtocolServer::new()
    }
}

impl Default for UnisonProtocol {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unison_creation() {
        let protocol = UnisonProtocol::new();
        assert_eq!(protocol.schemas.len(), 0);
    }

    #[test]
    fn test_parse_channel_schema() {
        let schema = r#"
protocol "test" version="1.0.0" {
    channel "ping" from="client" lifetime="persistent" {
        request "Ping" {
            field "message" type="string"
            returns "PingResponse" {
                field "reply" type="string"
            }
        }
    }
}
        "#;

        let mut protocol = UnisonProtocol::new();
        let result = protocol.load_schema(schema);
        if let Err(e) = &result {
            eprintln!("パースエラー: {:?}", e);
        }
        assert!(result.is_ok());
        assert_eq!(protocol.schemas.len(), 1);
    }

    #[test]
    fn test_parse_legacy_service_schema() {
        // 後方互換: パーサーは旧 service 構文もまだ受け付ける
        let schema = r#"
protocol "test" version="1.0.0" {
    service "TestService" {
        method "test_method" {
            request {
                field "id" type="string"
            }
            response {
                field "id" type="string"
            }
        }
    }
}
        "#;

        let mut protocol = UnisonProtocol::new();
        let result = protocol.load_schema(schema);
        if let Err(e) = &result {
            eprintln!("パースエラー: {:?}", e);
        }
        assert!(result.is_ok());
    }

    #[test]
    fn test_client_server_creation() {
        let protocol = UnisonProtocol::new();
        let _client = protocol.create_client().unwrap();
        let _server = protocol.create_server();
        // パニックが発生しなければテスト成功
    }
}
