use anyhow::Result;
use thiserror::Error;

pub mod schema;
pub mod types;

pub use schema::*;
pub use types::*;

/// Parser errors for Unison Protocol
#[derive(Error, Debug)]
pub enum ParseError {
    #[error("KDL parsing error: {0}")]
    Kdl(#[from] kdl::KdlError),
    #[error("Schema validation error: {0}")]
    Validation(String),
    #[error("Type error: {0}")]
    Type(String),
    #[error("Generic parsing error: {0}")]
    Generic(String),
    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error),
}

/// Main schema parser for KDL protocol definitions
pub struct SchemaParser;

impl SchemaParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse a KDL schema string into a ParsedSchema
    ///
    /// パース後に [`Channel::validate`] を全 channel に対して呼び、 datagram channel の
    /// `channel_id` 必須性等の semantic constraint を検証する。
    pub fn parse(&self, input: &str) -> Result<ParsedSchema> {
        // club-kdlを使ってパース
        let schema: ParsedSchema =
            club_kdl::from_str(input).map_err(|e| anyhow::anyhow!("KDL parsing error: {}", e))?;

        // Channel semantic validation (v0.10.0 で導入: datagram channel の channel_id 必須性等)
        if let Some(ref protocol) = schema.protocol {
            for channel in &protocol.channels {
                channel
                    .validate()
                    .map_err(|msg| anyhow::anyhow!("Schema validation: {}", msg))?;
            }
        }

        Ok(schema)
    }
}

impl Default for SchemaParser {
    fn default() -> Self {
        Self::new()
    }
}
