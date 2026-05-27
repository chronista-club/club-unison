//! UnisonMcp — MCP server tool surface (= 3 tools: ping / call / discover)
//!
//! - `unison_ping`: endpoint への疎通確認 (= probe 互換 escape hatch)
//! - `unison_call`: 任意 channel / method に payload 送信 (= probe 互換 escape hatch)
//! - `unison_discover`: `unison.discovery` channel 経由で server の protocol KDL を
//!   fetch、 channel / request 一覧 + schema 情報を返す (= NEW、 DynamicProtocol::fetch)
//!
//! 全 tool が endpoint と trust を引数で受け取る。 BridgeConfig に default が
//! 設定されていれば省略可能 (= UnisonBridge::resolve_* で fallback)。

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::bridge::UnisonBridge;
use crate::config::TrustMode;

// ---------------------------------------------------------------------------
// MCP server state
// ---------------------------------------------------------------------------

/// MCP server 本体。 内部に `UnisonBridge` を抱えて、 全 tool が共有する。
#[derive(Clone)]
pub struct UnisonMcp {
    bridge: Arc<UnisonBridge>,
    /// `#[tool_router]` macro 内部参照、 user code からは触らない
    #[allow(dead_code)]
    tool_router: ToolRouter<UnisonMcp>,
}

impl UnisonMcp {
    pub fn new(bridge: UnisonBridge) -> Self {
        Self {
            bridge: Arc::new(bridge),
            tool_router: Self::tool_router(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool argument schemas
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PingArgs {
    /// Unison サーバの endpoint URL (= 例: `quic://[::1]:7878`)。
    /// BridgeConfig に default endpoint があれば省略可。
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Trust anchor mode (= "skip" / "system")。 省略時は BridgeConfig の default、
    /// それも無ければ "skip"。
    #[serde(default)]
    pub trust: Option<TrustMode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CallArgs {
    /// Unison サーバの endpoint URL。 BridgeConfig に default endpoint があれば省略可。
    #[serde(default)]
    pub endpoint: Option<String>,

    /// 対象 channel 名 (= 例: `"unison.discovery"`、 `"chat"`)
    pub channel_name: String,

    /// 対象 method 名 (= KDL の `request "Name"` の Name)
    pub method: String,

    /// 送信する JSON payload
    pub payload: serde_json::Value,

    /// Trust mode (省略可)
    #[serde(default)]
    pub trust: Option<TrustMode>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiscoverArgs {
    /// Unison サーバの endpoint URL。 BridgeConfig に default endpoint があれば省略可。
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Trust mode (省略可)
    #[serde(default)]
    pub trust: Option<TrustMode>,
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// 共通: endpoint を resolve + ProtocolClient を build + connect する
async fn connect_client(
    bridge: &UnisonBridge,
    endpoint_arg: Option<&str>,
    trust_arg: Option<TrustMode>,
) -> Result<(unison::ProtocolClient, String), McpError> {
    use unison::ProtocolClient;
    use unison::network::quic::QuicClient;

    let endpoint = bridge.resolve_endpoint(endpoint_arg).ok_or_else(|| {
        McpError::invalid_request(
            "endpoint not provided and no default in BridgeConfig".to_string(),
            None,
        )
    })?;
    let trust = bridge.resolve_trust(trust_arg);

    let quic = QuicClient::builder()
        .trust_anchors(trust.to_anchors())
        .build()
        .map_err(|e| McpError::internal_error(format!("client init failed: {e}"), None))?;
    let client = ProtocolClient::new(quic);

    client
        .connect(endpoint)
        .await
        .map_err(|e| McpError::internal_error(format!("connect failed: {e}"), None))?;

    Ok((client, endpoint.to_string()))
}

#[tool_router]
impl UnisonMcp {
    /// Unison server への疎通確認 (= probe 互換 escape hatch)。
    #[tool(description = "Unison サーバへの疎通確認。 endpoint に接続して切断する。")]
    async fn unison_ping(
        &self,
        Parameters(args): Parameters<PingArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (_client, endpoint) =
            connect_client(&self.bridge, args.endpoint.as_deref(), args.trust).await?;
        let trust = self.bridge.resolve_trust(args.trust);
        let msg = format!("✅ connected to {endpoint} (trust={trust:?})");
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    /// 任意 channel.method を generic に叩く (= probe 互換 escape hatch、 schema 検証なし)。
    #[tool(
        description = "任意の Unison channel を open し、 method に payload を request として送信して response を取得する (= escape hatch、 schema 検証なし)"
    )]
    async fn unison_call(
        &self,
        Parameters(args): Parameters<CallArgs>,
    ) -> Result<CallToolResult, McpError> {
        let (client, _endpoint) =
            connect_client(&self.bridge, args.endpoint.as_deref(), args.trust).await?;

        let channel = client
            .open_channel(&args.channel_name)
            .await
            .map_err(|e| McpError::internal_error(format!("open_channel failed: {e}"), None))?;

        let response: serde_json::Value = channel
            .request(&args.method, &args.payload)
            .await
            .map_err(|e| McpError::internal_error(format!("request failed: {e}"), None))?;

        let result = serde_json::json!({
            "channel": args.channel_name,
            "method": args.method,
            "response": response,
        });
        Ok(CallToolResult::success(vec![Content::text(
            result.to_string(),
        )]))
    }

    /// Unison server の protocol KDL を `unison.discovery` channel 経由で fetch、
    /// channel / request 一覧 + schema metadata を返す (= NEW、 Hailing α P2-Rust 経由)。
    #[tool(
        description = "Unison サーバの protocol KDL を `unison.discovery` channel 経由で runtime fetch し、 channel / request 一覧と schema metadata (version / hash / codecs) を返す。 AI agent が server を初見で探索する用。"
    )]
    async fn unison_discover(
        &self,
        Parameters(args): Parameters<DiscoverArgs>,
    ) -> Result<CallToolResult, McpError> {
        use unison::network::DynamicProtocol;

        let (client, endpoint) =
            connect_client(&self.bridge, args.endpoint.as_deref(), args.trust).await?;
        let client = Arc::new(client);

        let proto = DynamicProtocol::fetch(client.clone())
            .await
            .map_err(|e| McpError::internal_error(format!("discovery fetch failed: {e}"), None))?;

        // channel / request 一覧を summary 化
        let channels: Vec<serde_json::Value> = proto
            .registry()
            .channels()
            .map(|ch| {
                let requests: Vec<&str> = ch.requests.iter().map(|r| r.name.as_str()).collect();
                let events: Vec<&str> = ch.events.iter().map(|e| e.name.as_str()).collect();
                serde_json::json!({
                    "name": ch.name,
                    "from": format!("{:?}", ch.from).to_lowercase(),
                    "lifetime": format!("{:?}", ch.lifetime).to_lowercase(),
                    "backend": format!("{:?}", ch.backend()).to_lowercase(),
                    "requests": requests,
                    "events": events,
                })
            })
            .collect();

        let summary = serde_json::json!({
            "endpoint": endpoint,
            "protocol_name": proto.protocol_name(),
            "version": proto.version(),
            "namespace": proto.registry().protocol_namespace(),
            "hash": proto.hash(),
            "codecs": proto.codecs(),
            "channels": channels,
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&summary).unwrap_or_else(|_| summary.to_string()),
        )]))
    }
}

// ---------------------------------------------------------------------------
// ServerHandler
// ---------------------------------------------------------------------------

#[tool_handler]
impl ServerHandler for UnisonMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "MCP bridge for Unison Protocol. \
                 Use `unison_discover` to explore an unknown server's schema (= channel / request 一覧 + version / hash)、 \
                 `unison_call` to send any request to a channel (= escape hatch、 schema 検証なし)、 \
                 `unison_ping` to verify connectivity. \
                 Future versions will synthesize typed tools per channel.method from the discovered schema.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BridgeConfig;

    #[test]
    fn server_builds_with_default_bridge() {
        let _server = UnisonMcp::new(UnisonBridge::new(BridgeConfig::default()));
    }
}
