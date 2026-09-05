use thiserror::Error;

pub mod schema;

pub use schema::*;

/// KDL スキーマの parse / 検証エラー。
#[derive(Error, Debug)]
pub enum ParseError {
    /// KDL 文法として読めない、 あるいは [`ParsedSchema`] の形に合わない。
    #[error("KDL parsing error: {0}")]
    Kdl(String),
    /// 文法は通ったが、 スキーマの意味的制約に反する
    /// (= datagram channel の `channel_id` 欠落、 `readonly` と `destructive` の同時宣言 等)。
    #[error("Schema validation error: {0}")]
    Validation(String),
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
    pub fn parse(&self, input: &str) -> Result<ParsedSchema, ParseError> {
        let schema: ParsedSchema =
            club_kdl::from_str(input).map_err(|e| ParseError::Kdl(e.to_string()))?;

        // Channel semantic validation (v0.10.0 で導入: datagram channel の channel_id 必須性等)
        if let Some(ref protocol) = schema.protocol {
            for channel in &protocol.channels {
                channel.validate().map_err(ParseError::Validation)?;
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
