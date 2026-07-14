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
    ErrorData as McpError, Peer, RoleServer, ServerHandler,
    handler::server::{common::schema_for_type, wrapper::Parameters},
    model::*,
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::bridge::{DiscoveredProtocol, UnisonBridge};
use crate::config::TrustMode;
use crate::mapping;

/// remote KDL から synthesize する tool 数の上限 (= tool-list flooding / 資源消費の防御)
const MAX_SYNTHESIZED_TOOLS: usize = 256;

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
        // annotations: endpoint が任意指定できる = 外部世界に開いている (open_world)。
        // ping / discover は server 状態を変えない (read_only + idempotent)。
        // call は任意 payload を送る escape hatch なので hint なし (= client は
        // spec default の「destructive かもしれない」扱いで確認を挟める)。
        let static_tools = vec![
            Tool::new(
                Cow::Borrowed("unison_ping"),
                Cow::Borrowed(
                    "Verify connectivity to a Unison server. Connects then disconnects, returning a success message.",
                ),
                schema_for_type::<PingArgs>(),
            )
            .annotate(
                ToolAnnotations::new()
                    .read_only(true)
                    .idempotent(true)
                    .open_world(true),
            ),
            Tool::new(
                Cow::Borrowed("unison_call"),
                Cow::Borrowed(
                    "Generic escape hatch: open any channel on a Unison server, send a typed method+payload, return the response. No schema validation.",
                ),
                schema_for_type::<CallArgs>(),
            )
            .annotate(ToolAnnotations::new().open_world(true)),
            Tool::new(
                Cow::Borrowed("unison_discover"),
                Cow::Borrowed(
                    "Fetch the protocol KDL from a Unison server via the `unison.discovery` channel. Returns channel/request listing + version/hash/codecs. When targeting the configured default endpoint, refreshes the synthesized tool set (emits tools/list_changed on schema change).",
                ),
                schema_for_type::<DiscoverArgs>(),
            )
            .annotate(
                ToolAnnotations::new()
                    .read_only(true)
                    .idempotent(true)
                    .open_world(true),
            ),
        ];
        Self {
            bridge: Arc::new(bridge),
            static_tools,
        }
    }

    /// 全 tool (= static + synthesized) を列挙する。 ServerHandler::list_tools と
    /// integration test 両方が呼ぶ entry。
    ///
    /// Tool name collision (= 異なる channel が同じ normalized 名に潰れる、 例:
    /// `chat.send` と `chat_send` 両方 → `unison_chat_send_X`) は warn log を吐いて
    /// **先に登録された方を優先** (= first-wins)。 silent wrong dispatch を防ぐ。
    pub fn all_tools(&self) -> Vec<Tool> {
        use std::collections::HashSet;

        let mut tools = self.static_tools.clone();
        let mut seen: HashSet<String> = self
            .static_tools
            .iter()
            .map(|t| t.name.to_string())
            .collect();

        if let Some(disc) = self.bridge.discovered() {
            let mut synthesized = 0usize;
            'outer: for channel in disc.proto.registry().channels() {
                for request in &channel.requests {
                    // remote KDL 由来の tool 数に上限を設ける (= 悪意ある / 巨大な
                    // discovery server による tool-list flooding / 資源消費を防ぐ)。
                    if synthesized >= MAX_SYNTHESIZED_TOOLS {
                        tracing::warn!(
                            cap = MAX_SYNTHESIZED_TOOLS,
                            "synthesized tool cap reached; remaining channel.requests are \
                             not exposed (large or hostile discovery schema?)"
                        );
                        break 'outer;
                    }
                    let tool = mapping::synthesize_tool(&channel.name, request);
                    let name = tool.name.to_string();
                    if !seen.insert(name.clone()) {
                        tracing::warn!(
                            tool = %name,
                            channel = %channel.name,
                            method = %request.name,
                            "tool name collision detected — skipping duplicate (first-wins); \
                             check that no two channels normalize to the same MCP tool name"
                        );
                        continue;
                    }
                    tools.push(tool);
                    synthesized += 1;
                }
            }
        }
        tools
    }

    /// MCP transport context を要らない tool dispatch (= integration test からも
    /// 直接呼べる)。 peer 無し = elicitation / list_changed 通知はスキップされる。
    pub async fn invoke_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, McpError> {
        self.invoke_tool_with_peer(name, args, None).await
    }

    /// peer 付き tool dispatch (= ServerHandler::call_tool の本体)。 peer は
    /// endpoint 未設定時の elicitation と、 discovery refresh 時の
    /// `tools/list_changed` 通知に使う。
    pub async fn invoke_tool_with_peer(
        &self,
        name: &str,
        args: serde_json::Value,
        peer: Option<&Peer<RoleServer>>,
    ) -> Result<CallToolResult, McpError> {
        match name {
            "unison_ping" => {
                let Parameters(args) = parse_params::<PingArgs>(args)?;
                handle_ping(&self.bridge, args, peer).await
            }
            "unison_call" => {
                let Parameters(args) = parse_params::<CallArgs>(args)?;
                handle_call(&self.bridge, args, peer).await
            }
            "unison_discover" => {
                let Parameters(args) = parse_params::<DiscoverArgs>(args)?;
                handle_discover(&self.bridge, args, peer).await
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
        let (channel_name, method) = resolve_synth_tool(&disc, name)?;
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

/// endpoint 未設定時に client へ質問する elicitation の回答 schema。
/// MCP elicitation は flat な primitive object のみ許可 (= `ElicitationSafe`)。
#[derive(Debug, Deserialize, JsonSchema)]
struct EndpointAnswer {
    /// Unison server endpoint URL (= 例: `quic://[::1]:7878`)
    endpoint: String,
}
rmcp::elicit_safe!(EndpointAnswer);

/// client に endpoint をその場で質問する (= MCP elicitation)。
/// client が elicitation 非対応 / 拒否 / cancel / 失敗は None を返し、
/// 呼び出し側は従来どおり invalid_request エラーへ fallback する。
async fn elicit_endpoint(peer: Option<&Peer<RoleServer>>) -> Option<String> {
    let peer = peer?;
    let supports = peer
        .peer_info()
        .is_some_and(|info| info.capabilities.elicitation.is_some());
    if !supports {
        return None;
    }
    match peer
        .elicit::<EndpointAnswer>(
            "No Unison endpoint is configured. Enter the server endpoint URL (e.g. quic://[::1]:7878).",
        )
        .await
    {
        Ok(Some(ans)) => {
            let endpoint = ans.endpoint.trim().to_string();
            if endpoint.is_empty() { None } else { Some(endpoint) }
        }
        // decline / cancel はユーザーの意思 = そのままエラー路線へ
        Ok(None) => None,
        Err(e) => {
            tracing::debug!(error = %e, "endpoint elicitation failed; falling back to error");
            None
        }
    }
}

/// 共通: endpoint を resolve + ProtocolClient を build + connect する。
/// endpoint が arg にも config にも無い場合、 elicitation 対応 client には
/// その場で質問する (= [`elicit_endpoint`])。
async fn connect_client(
    bridge: &UnisonBridge,
    endpoint_arg: Option<&str>,
    trust_arg: Option<TrustMode>,
    peer: Option<&Peer<RoleServer>>,
) -> Result<(unison::ProtocolClient, String), McpError> {
    use unison::ProtocolClient;
    use unison::network::quic::QuicClient;

    let endpoint: String = match bridge.resolve_endpoint(endpoint_arg) {
        Some(e) => e.to_string(),
        None => elicit_endpoint(peer).await.ok_or_else(|| {
            McpError::invalid_request(
                "endpoint not provided and no default in BridgeConfig".to_string(),
                None,
            )
        })?,
    };
    let trust = bridge.resolve_trust(trust_arg, &endpoint);

    let quic = QuicClient::builder()
        .trust_anchors(trust.to_anchors())
        .build()
        .map_err(|e| McpError::internal_error(format!("client init failed: {e}"), None))?;
    let client = ProtocolClient::new(quic);

    client
        .connect(&endpoint)
        .await
        .map_err(|e| McpError::internal_error(format!("connect failed: {e}"), None))?;

    Ok((client, endpoint))
}

async fn handle_ping(
    bridge: &UnisonBridge,
    args: PingArgs,
    peer: Option<&Peer<RoleServer>>,
) -> Result<CallToolResult, McpError> {
    let (_client, endpoint) =
        connect_client(bridge, args.endpoint.as_deref(), args.trust, peer).await?;
    let trust = bridge.resolve_trust(args.trust, &endpoint);
    Ok(CallToolResult::structured(serde_json::json!({
        "connected": true,
        "endpoint": endpoint,
        "trust": format!("{trust:?}").to_lowercase(),
    })))
}

async fn handle_call(
    bridge: &UnisonBridge,
    args: CallArgs,
    peer: Option<&Peer<RoleServer>>,
) -> Result<CallToolResult, McpError> {
    let (client, _endpoint) =
        connect_client(bridge, args.endpoint.as_deref(), args.trust, peer).await?;

    let channel = client
        .open_channel(&args.channel_name)
        .await
        .map_err(|e| McpError::internal_error(format!("open_channel failed: {e}"), None))?;

    let response: serde_json::Value = channel
        .request(&args.method, &args.payload)
        .await
        .map_err(|e| McpError::internal_error(format!("request failed: {e}"), None))?;

    // escape hatch は output_schema を宣言しないので、 channel/method 込みの
    // wrapper object を structured で返す (= 常に object になる)。
    Ok(CallToolResult::structured(serde_json::json!({
        "channel": args.channel_name,
        "method": args.method,
        "response": response,
    })))
}

async fn handle_discover(
    bridge: &UnisonBridge,
    args: DiscoverArgs,
    peer: Option<&Peer<RoleServer>>,
) -> Result<CallToolResult, McpError> {
    use unison::network::DynamicProtocol;

    let (client, endpoint) =
        connect_client(bridge, args.endpoint.as_deref(), args.trust, peer).await?;
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

    let hash = proto.hash().to_string();
    let mut summary = serde_json::json!({
        "endpoint": endpoint,
        "protocol_name": proto.protocol_name(),
        "version": proto.version(),
        "namespace": proto.registry().protocol_namespace(),
        "hash": hash,
        "codecs": proto.codecs(),
        "channels": channels,
        "refreshed": false,
    });

    // default endpoint への discover は bridge の discovery を refresh する
    // (= server の schema 進化に MCP session 中に追従する live bridge)。
    // 明示的に別 endpoint を指定した discover は one-off 照会で state を触らない。
    if bridge.resolve_endpoint(None) == Some(endpoint.as_str()) {
        let old_hash = bridge.set_discovered(DiscoveredProtocol {
            proto: Arc::new(proto),
        });
        summary["refreshed"] = serde_json::Value::Bool(true);
        if old_hash.as_deref() != Some(hash.as_str()) {
            tracing::info!(hash = %hash, "protocol schema changed — synthesized tool set refreshed");
            if let Some(peer) = peer
                && let Err(e) = peer.notify_tool_list_changed().await
            {
                tracing::warn!(error = %e, "tools/list_changed notification failed");
            }
        }
    }

    Ok(CallToolResult::structured(summary))
}

/// Synthesized typed tool の dispatch。 bridge.discovered() の DynamicProtocol 経由で
/// channel.request を実行し、 schema validation の error は MCP error として返す。
/// tool_name → (channel, method) を解決する。
///
/// `all_tools()` の列挙と **同一の first-wins** ロジックで、 同じ synthesized 名を
/// 生む最初の (channel, request) を返す。 list と dispatch が単一の解決規則を共有
/// するため、 名前衝突 (= 異なる channel が同じ正規 tool 名に潰れる) 時でも
/// 「list に出た tool」 と「dispatch される channel」 が必ず一致する
/// (= silent wrong dispatch を構造的に排除)。 一致が無ければ `None`。
fn resolve_synth_tool(disc: &DiscoveredProtocol, tool_name: &str) -> Option<(String, String)> {
    for channel in disc.proto.registry().channels() {
        for request in &channel.requests {
            if mapping::synth_tool_name(&channel.name, &request.name) == tool_name {
                return Some((channel.name.clone(), request.name.clone()));
            }
        }
    }
    None
}

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

    let (channel_name, method) = resolve_synth_tool(&disc, tool_name)
        .ok_or(McpError::method_not_found::<CallToolRequestMethod>())?;

    let chan = disc
        .proto
        .open_channel(&channel_name)
        .await
        .map_err(|e| McpError::internal_error(format!("open_channel failed: {e}"), None))?;

    let response = chan.request(&method, arguments).await.map_err(|e| {
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

    // structured_content は response そのもの (= KDL `returns` から合成した
    // output_schema と形が一致する)。 channel/method は tool 名が既に示している。
    // returns 未定義 channel の非 object response のみ wrapper で object 化する
    // (= MCP spec: structuredContent は object)。
    let structured = if response.is_object() {
        response
    } else {
        serde_json::json!({ "response": response })
    };
    Ok(CallToolResult::structured(structured))
}

// ---------------------------------------------------------------------------
// ServerHandler 実装 (= 手動)
// ---------------------------------------------------------------------------

impl ServerHandler for UnisonMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_instructions(
            "MCP bridge for Unison Protocol. Static escape hatch tools: \
                 `unison_ping` / `unison_call` / `unison_discover`. \
                 If a default endpoint is configured (= unison.json), synthesized typed tools \
                 named `unison_<channel>_<method>` are also exposed for each channel.request \
                 in the discovered KDL schema. Synthesized tools are payload-validated against \
                 the server's schema before dispatch (= fail-fast on type mismatch), declare \
                 output_schema from the KDL `returns` block, and return results as \
                 structuredContent (text mirror included). Calling `unison_discover` against \
                 the default endpoint refreshes the synthesized tool set (tools/list_changed).",
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
        context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let args_value = serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        async move {
            self.invoke_tool_with_peer(request.name.as_ref(), args_value, Some(&context.peer))
                .await
        }
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
        let names: Vec<&str> = server
            .static_tools
            .iter()
            .map(|t| t.name.as_ref())
            .collect();
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
    async fn static_tools_have_annotations() {
        let bridge = UnisonBridge::new(BridgeConfig::default()).await.unwrap();
        let server = UnisonMcp::new(bridge);

        let ping = server.find_tool("unison_ping").unwrap();
        let ann = ping.annotations.expect("ping annotations");
        assert_eq!(ann.read_only_hint, Some(true));
        assert_eq!(ann.idempotent_hint, Some(true));
        assert_eq!(ann.open_world_hint, Some(true));

        let discover = server.find_tool("unison_discover").unwrap();
        let ann = discover.annotations.expect("discover annotations");
        assert_eq!(ann.read_only_hint, Some(true));

        // call は escape hatch = read_only/destructive を主張しない (spec default 扱い)
        let call = server.find_tool("unison_call").unwrap();
        let ann = call.annotations.expect("call annotations");
        assert_eq!(ann.read_only_hint, None);
        assert_eq!(ann.open_world_hint, Some(true));
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
