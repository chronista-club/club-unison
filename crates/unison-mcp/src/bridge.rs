//! UnisonBridge — shared state for the MCP bridge.
//!
//! P3b で stateful 化 (= 起動時に config endpoint へ eagerly connect + `DynamicProtocol`
//! fetch、 schema を `discovered` field に保持)。 これにより `tools.rs` の `list_tools`
//! が synthesized typed tools を返せるようになる。
//!
//! Discovery failure (= endpoint 未指定 / 接続失敗 / KDL parse 失敗) は warn log
//! で continue、 static escape hatch tools (ping/call/discover) のみで動作。 これに
//! より agent 起動時に 「discovery server がまだ準備中」 の場合でも MCP server 自身は
//! 起動できる (= 部分動作の resilience)。

use std::sync::Arc;

use anyhow::Result;

use unison::ProtocolClient;
use unison::network::DynamicProtocol;
use unison::network::quic::QuicClient;

use crate::config::{BridgeConfig, TrustMode};

/// MCP bridge 全体の state。
pub struct UnisonBridge {
    config: BridgeConfig,
    /// 起動時に config.endpoint に対して fetch した DynamicProtocol。
    /// None = endpoint 未設定 or 接続失敗 (= synthesized tools 不可、 static のみ)。
    discovered: Option<DiscoveredProtocol>,
}

/// 起動時 discovery 結果
pub struct DiscoveredProtocol {
    pub proto: Arc<DynamicProtocol>,
}

impl UnisonBridge {
    /// 非同期 constructor。 config.endpoint があれば eagerly connect + fetch、
    /// 失敗時は warn log で continue (= static tools のみで動作)。
    pub async fn new(config: BridgeConfig) -> Result<Self> {
        let discovered = match config.endpoint.clone() {
            Some(endpoint) => {
                let trust = config.trust.unwrap_or_else(|| default_trust_for(&endpoint));
                match try_discover(&endpoint, trust).await {
                    Ok(proto) => {
                        let channel_count = proto.registry().channels().count();
                        tracing::info!(
                            endpoint = %endpoint,
                            protocol = %proto.protocol_name(),
                            version = %proto.version(),
                            channels = channel_count,
                            "discovery succeeded — synthesized tools available"
                        );
                        Some(DiscoveredProtocol {
                            proto: Arc::new(proto),
                        })
                    }
                    Err(e) => {
                        tracing::warn!(
                            endpoint = %endpoint,
                            error = %e,
                            "discovery failed; serving static escape hatch tools only"
                        );
                        None
                    }
                }
            }
            None => {
                tracing::info!(
                    "no default endpoint in config; serving static escape hatch tools only"
                );
                None
            }
        };
        Ok(Self { config, discovered })
    }

    /// Discovered protocol への参照 (= synthesized tools 用)
    pub fn discovered(&self) -> Option<&DiscoveredProtocol> {
        self.discovered.as_ref()
    }

    /// Tool arg で endpoint が未指定の場合、 config の default endpoint を返す。
    /// 両方とも未指定なら None。
    pub fn resolve_endpoint<'a>(&'a self, arg: Option<&'a str>) -> Option<&'a str> {
        arg.or(self.config.endpoint.as_deref())
    }

    /// trust を解決する。 優先順位: tool arg > config > endpoint 由来の default。
    ///
    /// 明示指定が無い場合、 endpoint が loopback なら [`TrustMode::Skip`]
    /// (= dev self-signed server 向け)、 それ以外 (= remote) なら
    /// [`TrustMode::System`] を default にする。 これにより remote への
    /// 「無言の証明書検証スキップ」を防ぐ (= secure-by-default)。 connect() 側の
    /// 「Skip は loopback 限定」 gate と同じ哲学。
    pub fn resolve_trust(&self, arg: Option<TrustMode>, endpoint: &str) -> TrustMode {
        arg.or(self.config.trust)
            .unwrap_or_else(|| default_trust_for(endpoint))
    }
}

/// endpoint の host 部分が loopback (= `localhost` / `127.x` / `::1`) か判定する。
///
/// scheme prefix と port を剥がして host を取り出す。 厳密な解決は connect() 側の
/// `addr.ip().is_loopback()` gate が行うため、 ここは default 選択用の heuristic。
fn is_loopback_endpoint(endpoint: &str) -> bool {
    let host = endpoint
        .strip_prefix("quic://")
        .or_else(|| endpoint.strip_prefix("https://"))
        .or_else(|| endpoint.strip_prefix("http://"))
        .unwrap_or(endpoint);

    // [::1]:port / [::1] → bracket 内
    let host = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else if let Some(colon) = host.rfind(':') {
        // host:port (host 側に ':' が無い = IPv6 リテラルではない場合のみ port を剥がす)
        if host[..colon].contains(':') {
            host
        } else {
            &host[..colon]
        }
    } else {
        host
    };

    host == "localhost"
        || host == "::1"
        || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
        || host.parse::<std::net::Ipv6Addr>().is_ok_and(|ip| ip.is_loopback())
}

/// 明示指定が無いときの endpoint 由来 default trust。
fn default_trust_for(endpoint: &str) -> TrustMode {
    if is_loopback_endpoint(endpoint) {
        TrustMode::Skip
    } else {
        TrustMode::System
    }
}

/// 内部: endpoint + trust から DynamicProtocol を build する。
///
/// 全体を 3 秒 timeout で wrap (= QUIC の `max_idle_timeout=60s` を待たず、
/// bridge 起動時に discovery server が unreachable な場合の wall を圧縮)。
/// startup-time discovery は best-effort、 失敗時は static escape hatch のみで継続。
async fn try_discover(endpoint: &str, trust: TrustMode) -> Result<DynamicProtocol> {
    use tokio::time::{Duration, timeout};
    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);

    timeout(DISCOVERY_TIMEOUT, async {
        let quic = QuicClient::builder()
            .trust_anchors(trust.to_anchors())
            .build()?;
        let client = Arc::new(ProtocolClient::new(quic));
        client.connect(endpoint).await?;
        let proto = DynamicProtocol::fetch(client.clone()).await?;
        anyhow::Ok(proto)
    })
    .await
    .map_err(|_| anyhow::anyhow!("discovery timeout after {DISCOVERY_TIMEOUT:?} for {endpoint}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_without_endpoint_starts_without_discovery() {
        let bridge = UnisonBridge::new(BridgeConfig::default()).await.unwrap();
        assert!(bridge.discovered().is_none());
    }

    #[tokio::test]
    async fn bridge_with_bad_endpoint_logs_warn_and_continues() {
        // 存在しない endpoint = discovery 失敗、 でも bridge は build 成功
        let bridge = UnisonBridge::new(BridgeConfig {
            endpoint: Some("quic://127.0.0.1:1".to_string()), // 高確率で open してない
            trust: Some(TrustMode::Skip),
        })
        .await
        .unwrap();
        assert!(
            bridge.discovered().is_none(),
            "discovery should fail silently, bridge should still build"
        );
    }

    #[test]
    fn endpoint_resolution_arg_priority() {
        // 純粋 sync な resolve は async constructor を経由せず構築できるよう、
        // 直接 struct を組む (= test only)
        let bridge = UnisonBridge {
            config: BridgeConfig {
                endpoint: Some("default".to_string()),
                trust: None,
            },
            discovered: None,
        };
        assert_eq!(bridge.resolve_endpoint(Some("override")), Some("override"));
        assert_eq!(bridge.resolve_endpoint(None), Some("default"));
    }

    #[test]
    fn endpoint_resolution_empty_returns_none() {
        let bridge = UnisonBridge {
            config: BridgeConfig::default(),
            discovered: None,
        };
        assert_eq!(bridge.resolve_endpoint(None), None);
    }

    #[test]
    fn trust_resolution_arg_priority() {
        let bridge = UnisonBridge {
            config: BridgeConfig {
                endpoint: None,
                trust: Some(TrustMode::System),
            },
            discovered: None,
        };
        // arg / config の明示は endpoint に関係なく優先される
        assert_eq!(
            bridge.resolve_trust(Some(TrustMode::Skip), "quic://example.com:7878"),
            TrustMode::Skip
        );
        assert_eq!(
            bridge.resolve_trust(None, "quic://example.com:7878"),
            TrustMode::System
        );
    }

    #[test]
    fn trust_default_is_skip_for_loopback() {
        let bridge = UnisonBridge {
            config: BridgeConfig::default(),
            discovered: None,
        };
        // 明示なし + loopback → Skip (dev ergonomics)
        assert_eq!(
            bridge.resolve_trust(None, "quic://[::1]:7878"),
            TrustMode::Skip
        );
        assert_eq!(
            bridge.resolve_trust(None, "quic://127.0.0.1:7878"),
            TrustMode::Skip
        );
        assert_eq!(
            bridge.resolve_trust(None, "quic://localhost:7878"),
            TrustMode::Skip
        );
    }

    #[test]
    fn trust_default_is_system_for_remote() {
        let bridge = UnisonBridge {
            config: BridgeConfig::default(),
            discovered: None,
        };
        // 明示なし + remote → System (secure-by-default、 silent skip を防ぐ)
        assert_eq!(
            bridge.resolve_trust(None, "quic://cp.fleetstage.cloud:7878"),
            TrustMode::System
        );
        assert_eq!(
            bridge.resolve_trust(None, "quic://[2001:db8::1]:7878"),
            TrustMode::System
        );
    }
}
