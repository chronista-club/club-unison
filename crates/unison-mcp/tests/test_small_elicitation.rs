//! Small x Integration: endpoint 未設定時の問い返し (MCP elicitation) が、 答えの無い
//! client で止まり続けない。
//!
//! elicitation の capability を出していても、 答える人がいない client がある
//! (= agent が動かす session で、 問い返しが画面に出ない)。 期限が来たら、 直し方の
//! 分かる error (`endpoint` を渡すか `--config` を) に落ちること。
//!
//! in-memory の MCP client / server ペアで確かめる (実 QUIC は使わない)。

use std::time::{Duration, Instant};

use anyhow::Result;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult,
    Implementation,
};
use rmcp::service::RequestContext;
use rmcp::{ClientHandler, ErrorData as McpError, RoleClient, ServiceExt};
use tokio::time::timeout;

use unison_mcp::bridge::UnisonBridge;
use unison_mcp::config::BridgeConfig;
use unison_mcp::tools::UnisonMcp;

/// elicitation に対応すると名乗るが、 問い返しに答えない client。
struct SilentClient;

impl ClientHandler for SilentClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::builder().enable_elicitation().build(),
            Implementation::new("silent-client", "0.0.0"),
        )
    }

    async fn create_elicitation(
        &self,
        _request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, McpError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn discover_without_endpoint_falls_back_to_error_when_nobody_answers() -> Result<()> {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);

    // endpoint 未設定の bridge。 問い返しを待つのは 200ms まで。
    let bridge = UnisonBridge::new(BridgeConfig::default())
        .await?
        .with_elicitation_timeout(Duration::from_millis(200));
    let server = tokio::spawn(async move { UnisonMcp::new(bridge).serve(server_io).await });
    let client = SilentClient.serve(client_io).await?;
    let _server = server.await??;

    let started = Instant::now();
    let result = timeout(
        Duration::from_secs(5),
        client.call_tool(CallToolRequestParams::new("unison_discover")),
    )
    .await
    .expect("答えの無い問い返しで、 tool の呼び出しが止まり続けてはいけない");

    let error = result.expect_err("endpoint が無いので error になるべき");
    let message = error.to_string();
    assert!(
        message.contains("no endpoint") && message.contains("--config"),
        "直し方の分かる文面に落ちるべき: {message}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "問い返しを期限まで待ってから落ちる"
    );

    client.cancel().await?;
    Ok(())
}
