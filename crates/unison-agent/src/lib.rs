//! # Unison Agent
//!
//! Claude Agent SDKをUnisonプロトコル上で動作させるためのRust実装
//!
//! ## 機能
//!
//! - Claude Agent SDKのRust統合
//! - 非同期ストリーミング対応
//! - 型安全なエージェント操作
//!
//! ## 使用例
//!
//! ```no_run
//! use unison_agent::AgentClient;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = AgentClient::new();
//!     let response = client.query("Hello, Claude!").await?;
//!     println!("Response: {}", response);
//!     Ok(())
//! }
//! ```

pub mod agent;
pub mod client;
pub mod error;
pub mod tools;

pub use agent::AgentClient;
pub use error::{AgentError, Result};
pub use tools::UnisonTools;
