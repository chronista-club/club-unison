//! UnisonBridge — shared state for the MCP bridge.
//!
//! 現状 (= P3a scaffold) は stateless: BridgeConfig だけを抱えて、 tool 毎に
//! independent な `ProtocolClient` を build する (= probe と同じ pattern)。
//!
//! P3b で synthesized typed tools が入る時に、 起動時に config endpoint へ
//! eagerly connect し `DynamicProtocol` を抱える形に拡張する想定。

use crate::config::{BridgeConfig, TrustMode};

/// MCP bridge 全体の state。
#[derive(Debug, Clone)]
pub struct UnisonBridge {
    config: BridgeConfig,
}

impl UnisonBridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self { config }
    }

    /// Tool arg で endpoint が未指定の場合、 config の default endpoint を返す。
    /// 両方とも未指定なら None。
    pub fn resolve_endpoint<'a>(&'a self, arg: Option<&'a str>) -> Option<&'a str> {
        arg.or_else(|| self.config.endpoint.as_deref())
    }

    /// Tool arg で trust が未指定の場合、 config の default trust を返す。
    /// 両方とも未指定なら `TrustMode::Skip` (= dev default)。
    pub fn resolve_trust(&self, arg: Option<TrustMode>) -> TrustMode {
        arg.or(self.config.trust).unwrap_or(TrustMode::Skip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_resolution_arg_priority() {
        let bridge = UnisonBridge::new(BridgeConfig {
            endpoint: Some("default".to_string()),
            trust: None,
        });
        // arg が prio
        assert_eq!(bridge.resolve_endpoint(Some("override")), Some("override"));
        // arg が None なら config fallback
        assert_eq!(bridge.resolve_endpoint(None), Some("default"));
    }

    #[test]
    fn endpoint_resolution_empty_returns_none() {
        let bridge = UnisonBridge::new(BridgeConfig::default());
        assert_eq!(bridge.resolve_endpoint(None), None);
    }

    #[test]
    fn trust_resolution_arg_priority() {
        let bridge = UnisonBridge::new(BridgeConfig {
            endpoint: None,
            trust: Some(TrustMode::System),
        });
        // arg が prio
        assert_eq!(bridge.resolve_trust(Some(TrustMode::Skip)), TrustMode::Skip);
        // arg が None なら config
        assert_eq!(bridge.resolve_trust(None), TrustMode::System);
    }

    #[test]
    fn trust_resolution_defaults_to_skip() {
        let bridge = UnisonBridge::new(BridgeConfig::default());
        assert_eq!(bridge.resolve_trust(None), TrustMode::Skip);
    }
}
