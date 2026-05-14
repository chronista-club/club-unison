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
pub struct SchemaParser {
    #[allow(dead_code)]
    type_registry: TypeRegistry,
}

impl SchemaParser {
    pub fn new() -> Self {
        Self {
            type_registry: TypeRegistry::new(),
        }
    }

    /// Parse a KDL schema string into a ParsedSchema
    pub fn parse(&self, input: &str) -> Result<ParsedSchema> {
        // unison-kdlを使ってパース
        let schema: ParsedSchema =
            club_kdl::from_str(input).map_err(|e| anyhow::anyhow!("KDL parsing error: {}", e))?;

        Ok(schema)
    }
}

impl Default for SchemaParser {
    fn default() -> Self {
        Self::new()
    }
}
