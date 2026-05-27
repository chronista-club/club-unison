//! unison-mcp — MCP (Model Context Protocol) bridge for Unison Protocol.
//!
//! Binary crate の library 部分。 internal module を tests/ + 外部 caller に
//! pub re-export している。 詳細は各 module の doc 参照。

pub mod bridge;
pub mod config;
pub mod mapping;
pub mod tools;
