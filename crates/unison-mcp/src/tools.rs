//! UnisonMcp — MCP `ServerHandler` 実装。 static escape hatch tools + synthesized
//! typed tools の merged dispatch を行う。
//!
//! ## Tool 一覧
//!
//! ### Static escape hatch tools (= 常に available)
//!
//! - `unison_ping(endpoint?, trust?)`
//! - `unison_call(endpoint?, channel_name, method, payload, trust?)`
//! - `unison_discover(endpoint?, trust?)`
//!
//! ### Synthesized typed tools (= config endpoint で discovery 成功時のみ)
//!
//! - `unison_<channel_safe>_<method>(...)`  各 KDL `channel.request` から動的合成
//! - input_schema は KDL `field` から `mapping::field_type_to_schema` で生成
//!
//! 設計判断: `#[tool]` / `#[tool_router]` macro を撤去して `ServerHandler` を手動 impl。
//! 理由 = 動的 tool を `list_tools` で混ぜて返す必要があり、 macro は static tool に
//! 限定される。

use std::borrow::Cow;
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{common::schema_for_type, wrapper::Parameters},
    model::*,
    service::RequestContext,
    RoleServer,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::bridge::UnisonBridge;
use crate::config::TrustMode;
use crate::mapping;

// ---------------------------------------------------------------------------
// MCP server state
// ---------------------------------------------------------------------------

/// MCP server 本体。 内部に `UnisonBridge` を抱えて全 tool で共有する。
pub struct UnisonMcp {
    bridge: Arc<UnisonBridge>,
    /// 起動時に build した static tool 一覧 (= ping / call / discover)。
    /// synthesized tools は list_tools 呼出毎に bridge.discovered() から再構築する。
    static_tools: Vec<Tool>,
}

impl UnisonMcp {
    pub fn new(bridge: UnisonBridge) -> Self {
        let static_tools = vec![
            Tool::new(
                Cow::Borrowed("unison_ping"),
                Cow::Borrowed(
                    "Verify connectivity to a Unison server. Connects then disconnects, returning a success message.",
                ),
                schema_for_type::<PingArgs>(),
            ),
            Tool::new(
                Cow::Borrowed("unison_call"),
                Cow::Borrowed(
                    "Generic escape hatch: open any channel on a Unison server, send a typed method+payload, return the response. No schema validation.",
                ),
                schema_for_type::<CallArgs>(),
            ),
            Tool::new(
                Cow::Borrowed("unison_discover"),
                Cow::Borrowed(
                    "Fetch the protocol KDL from a Unison server via the `unison.discovery` channel. Returns channel/request listing + version/hash/codecs.",
                ),
                schema_for_type::<DiscoverArgs>(),
            ),
        ];
        Self {
            bridge: Arc::new(bridge),
            static_tools,
        }
    }

    /// 全 tool (= static + synthesized) を列挙する。 ServerHandler::list_tools と
    /// integration test 両方が呼ぶ entry。
    pub fn all_tools(&self) -> Vec<Tool> {
        let mut tools = self.static_tools.clone();
        if let Some(disc) = self.bridge.discovered() {
            for channel in disc.proto.registry().channels() {
                for request in &channel.requests {
                    tools.push(mapping::synthesize_tool(&channel.name, request));
                }
            }
        }
        tools
    }

    /// MCP transport context を要らない tool dispatch (= ServerHandler::call_tool の
    /// 本体、 integration test からも直接呼べる)。
    pub async fn invoke_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpError> {
        match name {
            "unison_ping" => {
                let Parameters(args) = parse_params::<PingArgs>(args)?;
                handle_ping(&self.bridge, args).await
            }
            "unison_call" => {
                let Parameters(args) = parse_params::<CallArgs>(args)?;
                handle_call(&self.bridge, args).await
            }
            "unison_discover" => {
                let Parameters(args) = parse_params::<DiscoverArgs>(args)?;
                handle_discover(&self.bridge, args).await
            }
            other => handle_synthesized(&self.bridge, other, args).await,
        }
    }

    /// Tool 名から該当する Tool を 1 件引く
    pub fn find_tool(&self, name: &str) -> Option<Tool> {
        if let Some(t) = self.static_tools.iter().find(|t| t.name.as_ref() == name) {
            return Some(t.clone());
        }
        let disc = self.bridge.discovered()?;
        let channel_names: Vec<&str> = disc
            .proto
            .registry()
            .channels()
            .map(|c| c.name.as_str())
            .collect();
        let (channel_name, method) = mapping::resolve_tool_name(name, channel_names)?;
        let request = disc
            .proto
            .registry()
            .request(&channel_name, &method)?
            .clone();
        Some(mapping::synthesize_tool(&channel_name, &request))
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
// Handler impls (= 各 tool の本体)
// ---------------------------------------------------------------------------

/// 共通: endpoint を resolve + ProtocolClient を build + connect する。
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

async fn handle_ping(
    bridge: &UnisonBridge,
    args: PingArgs,
) -> Result<CallToolResult, McpError> {
    let (_client, endpoint) =
        connect_client(bridge, args.endpoint.as_deref(), args.trust).await?;
    let trust = bridge.resolve_trust(args.trust);
    let msg = format!("✅ connected to {endpoint} (trust={trust:?})");
    Ok(CallToolResult::success(vec![Content::text(msg)]))
}

async fn handle_call(
    bridge: &UnisonBridge,
    args: CallArgs,
) -> Result<CallToolResult, McpError> {
    let (client, _endpoint) =
        connect_client(bridge, args.endpoint.as_deref(), args.trust).await?;

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

async fn handle_discover(
    bridge: &UnisonBridge,
    args: DiscoverArgs,
) -> Result<CallToolResult, McpError> {
    use unison::network::DynamicProtocol;

    let (client, endpoint) =
        connect_client(bridge, args.endpoint.as_deref(), args.trust).await?;
    let client = Arc::new(client);

    let proto = DynamicProtocol::fetch(client.clone())
        .await
        .map_err(|e| McpError::internal_error(format!("discovery fetch failed: {e}"), None))?;

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

/// Synthesized typed tool の dispatch。 bridge.discovered() の DynamicProtocol 経由で
/// channel.request を実行し、 schema validation の error は MCP error として返す。
async fn handle_synthesized(
    bridge: &UnisonBridge,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<CallToolResult, McpError> {
    let disc = bridge.discovered().ok_or_else(|| {
        McpError::invalid_request(
            format!(
                "no discovered protocol; synthesized tool `{tool_name}` cannot be served without a configured endpoint"
            ),
            None,
        )
    })?;

    let channel_names: Vec<&str> = disc
        .proto
        .registry()
        .channels()
        .map(|c| c.name.as_str())
        .collect();
    let (channel_name, method) =
        mapping::resolve_tool_name(tool_name, channel_names).ok_or_else(|| {
            McpError::method_not_found::<CallToolRequestMethod>()
        })?;

    let chan = disc
        .proto
        .open_channel(&channel_name)
        .await
        .map_err(|e| McpError::internal_error(format!("open_channel failed: {e}"), None))?;

    let response = chan
        .request(&method, arguments)
        .await
        .map_err(|e| {
            // DynamicError には Network / Validation / Registry / Serde がある。
            // Validation は invalid_request、 それ以外は internal_error にマップ。
            use unison::network::DynamicError;
            match e {
                DynamicError::Validation(v) => {
                    McpError::invalid_request(format!("validation: {v}"), None)
                }
                other => McpError::internal_error(format!("request failed: {other}"), None),
            }
        })?;

    let result = serde_json::json!({
        "channel": channel_name,
        "method": method,
        "response": response,
    });
    Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()),
    )]))
}

// ---------------------------------------------------------------------------
// ServerHandler 実装 (= 手動)
// ---------------------------------------------------------------------------

impl ServerHandler for UnisonMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "MCP bridge for Unison Protocol. Static escape hatch tools: \
                 `unison_ping` / `unison_call` / `unison_discover`. \
                 If a default endpoint is configured (= unison.json), synthesized typed tools \
                 named `unison_<channel>_<method>` are also exposed for each channel.request \
                 in the discovered KDL schema. Synthesized tools are payload-validated against \
                 the server's schema before dispatch (= fail-fast on type mismatch).",
            )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.all_tools();
        async move {
            Ok(ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.find_tool(name)
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let args_value =
            serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        async move { self.invoke_tool(request.name.as_ref(), args_value).await }
    }
}

/// JSON value を typed arg struct に deserialize、 失敗は invalid_request McpError
fn parse_params<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<Parameters<T>, McpError> {
    serde_json::from_value::<T>(value)
        .map(Parameters)
        .map_err(|e| McpError::invalid_request(format!("invalid arguments: {e}"), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BridgeConfig;

    #[tokio::test]
    async fn server_builds_with_default_bridge() {
        let bridge = UnisonBridge::new(BridgeConfig::default()).await.unwrap();
        let server = UnisonMcp::new(bridge);
        // static tools 3 つ
        assert_eq!(server.static_tools.len(), 3);
        let names: Vec<&str> = server.static_tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"unison_ping"));
        assert!(names.contains(&"unison_call"));
        assert!(names.contains(&"unison_discover"));
    }

    #[tokio::test]
    async fn all_tools_without_discovery_returns_only_static() {
        let bridge = UnisonBridge::new(BridgeConfig::default()).await.unwrap();
        let server = UnisonMcp::new(bridge);
        let tools = server.all_tools();
        assert_eq!(tools.len(), 3);
    }

    #[tokio::test]
    async fn find_tool_returns_static_by_name() {
        let bridge = UnisonBridge::new(BridgeConfig::default()).await.unwrap();
        let server = UnisonMcp::new(bridge);
        assert!(server.find_tool("unison_ping").is_some());
        assert!(server.find_tool("unison_call").is_some());
        assert!(server.find_tool("unison_discover").is_some());
        assert!(server.find_tool("ghost").is_none());
    }
}
