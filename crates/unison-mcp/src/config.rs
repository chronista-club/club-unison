//! BridgeConfig — `unison.json` config schema for `unison-mcp`.
//!
//! Optional config file。 指定された場合、 default endpoint と trust mode を提供する
//! (= tool arg で個別 override 可能)。
//!
//! # Example `unison.json`
//!
//! ```json
//! {
//!   "endpoint": "quic://[::1]:7878",
//!   "trust": "skip"
//! }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bridge 全体の config。 全 field optional、 omit 時は tool 引数に従う。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Default Unison endpoint (= 例: `"quic://[::1]:7878"`)、 tool arg で override 可能
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,

    /// Default trust mode (= `"skip"` / `"system"`)、 tool arg で override 可能
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustMode>,
}

/// Trust anchor mode (= unison::network::TrustAnchors に対応する serializable enum)
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum TrustMode {
    /// Skip cert verification (= dev only、 self-signed server 向け)
    #[default]
    Skip,
    /// OS / webpki-roots trust store (= public server 向け)
    System,
}

impl TrustMode {
    /// `unison::network::TrustAnchors` に変換
    pub fn to_anchors(self) -> unison::network::TrustAnchors {
        match self {
            Self::Skip => unison::network::TrustAnchors::SkipVerification,
            Self::System => unison::network::TrustAnchors::System,
        }
    }
}

impl BridgeConfig {
    /// JSON ファイルから config を読み込む
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let cfg: Self =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_serde_round_trip() {
        let cfg = BridgeConfig {
            endpoint: Some("quic://[::1]:7878".to_string()),
            trust: Some(TrustMode::System),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: BridgeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.endpoint.as_deref(), Some("quic://[::1]:7878"));
        assert_eq!(restored.trust, Some(TrustMode::System));
    }

    #[test]
    fn config_defaults_to_empty() {
        let cfg = BridgeConfig::default();
        assert!(cfg.endpoint.is_none());
        assert!(cfg.trust.is_none());
    }

    #[test]
    fn config_parses_partial_json() {
        // endpoint だけ指定、 trust は default
        let cfg: BridgeConfig =
            serde_json::from_str(r#"{ "endpoint": "quic://localhost:8000" }"#).unwrap();
        assert_eq!(cfg.endpoint.as_deref(), Some("quic://localhost:8000"));
        assert!(cfg.trust.is_none());
    }

    #[test]
    fn trust_mode_serializes_lowercase() {
        let json = serde_json::to_string(&TrustMode::Skip).unwrap();
        assert_eq!(json, r#""skip""#);
        let json = serde_json::to_string(&TrustMode::System).unwrap();
        assert_eq!(json, r#""system""#);
    }
}
