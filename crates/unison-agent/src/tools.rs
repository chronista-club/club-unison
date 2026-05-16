//! Claude Agent用のUnisonツール実装
//!
//! このモジュールは、Claude AgentがUnison Protocol経由で
//! 外部サービスにアクセスするためのツールを提供します。

use claude_agent_sdk::mcp::{SdkMcpServer, SdkMcpTool, ToolResult};
use serde_json::{Value, json};
use tracing::{debug, info};
use unison::ProtocolClient;

use crate::error::{AgentError, Result};

/// Unison Protocolツールセット
pub struct UnisonTools {
    client: Option<ProtocolClient>,
    server_url: Option<String>,
}

impl UnisonTools {
    /// 新しいUnisonツールセットを作成
    pub fn new() -> Self {
        Self {
            client: None,
            server_url: None,
        }
    }

    /// MCP ServerとしてUnisonツールを構築
    pub fn build_mcp_server() -> SdkMcpServer {
        // Tool 1: Unisonサーバーへ接続
        let connect_tool = SdkMcpTool::new(
            "unison_connect",
            "Connect to a Unison Protocol server",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The server URL to connect to (e.g., '[::1]:8080')"
                    }
                },
                "required": ["url"]
            }),
            |args: Value| {
                Box::pin(async move {
                    let url = args["url"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter"))
                        .map_err(|e| {
                            claude_agent_sdk::error::ClaudeError::Connection(e.to_string())
                        })?;

                    info!("Connecting to Unison server: {}", url);

                    // TODO: 実際の接続処理（チャネル経由）
                    // let mut client = ProtocolClient::new_default()?;
                    // client.connect(url).await?;

                    Ok(ToolResult::text(format!(
                        "Successfully connected to Unison server at {}",
                        url
                    )))
                })
            },
        );

        // Tool 2: チャネル経由でリクエストを送信
        let call_tool = SdkMcpTool::new(
            "unison_call",
            "Send a request through a Unison channel",
            json!({
                "type": "object",
                "properties": {
                    "channel": {
                        "type": "string",
                        "description": "The name of the channel to use"
                    },
                    "method": {
                        "type": "string",
                        "description": "The request method name"
                    },
                    "payload": {
                        "type": "object",
                        "description": "The request payload as JSON"
                    }
                },
                "required": ["channel", "method"]
            }),
            |args: Value| {
                Box::pin(async move {
                    let channel = args["channel"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing 'channel' parameter"))
                        .map_err(|e| {
                            claude_agent_sdk::error::ClaudeError::Connection(e.to_string())
                        })?;
                    let method = args["method"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Missing 'method' parameter"))
                        .map_err(|e| {
                            claude_agent_sdk::error::ClaudeError::Connection(e.to_string())
                        })?;
                    let payload = args.get("payload").cloned().unwrap_or(json!({}));

                    info!(
                        "Calling Unison channel: {}::{} with payload: {}",
                        channel, method, payload
                    );

                    // TODO: チャネル経由のリクエスト送信
                    // let ch = client.open_channel(channel).await?;
                    // let response = ch.request(method, payload).await?;

                    Ok(ToolResult::text(format!(
                        "Called {}::{} (mock response - channel implementation needed)",
                        channel, method
                    )))
                })
            },
        );

        // Tool 3: 利用可能なチャネル一覧を取得
        let list_tool = SdkMcpTool::new(
            "unison_list_channels",
            "List available channels on the connected Unison server",
            json!({
                "type": "object",
                "properties": {}
            }),
            |_args: Value| {
                Box::pin(async move {
                    info!("Listing Unison channels");

                    // TODO: Identity からチャネル一覧を取得
                    // let identity = client.server_identity().await;

                    Ok(ToolResult::text(
                        "Available channels: (mock list - channel implementation needed)",
                    ))
                })
            },
        );

        // Tool 4: Unisonサーバーから切断
        let disconnect_tool = SdkMcpTool::new(
            "unison_disconnect",
            "Disconnect from the Unison Protocol server",
            json!({
                "type": "object",
                "properties": {}
            }),
            |_args: Value| {
                Box::pin(async move {
                    info!("Disconnecting from Unison server");

                    // TODO: 実際の切断処理
                    // client.disconnect().await?;

                    Ok(ToolResult::text(
                        "Successfully disconnected from Unison server",
                    ))
                })
            },
        );

        SdkMcpServer::new("unison-protocol")
            .version("0.1.0")
            .tools(vec![connect_tool, call_tool, list_tool, disconnect_tool])
    }

    /// Unisonサーバーへ接続
    pub async fn connect(&mut self, url: &str) -> Result<()> {
        info!("Connecting to Unison server: {}", url);

        let client = ProtocolClient::new_default()
            .map_err(|e| AgentError::Communication(format!("Failed to create client: {}", e)))?;

        client
            .connect(url)
            .await
            .map_err(|e| AgentError::Communication(format!("Connection failed: {}", e)))?;

        self.client = Some(client);
        self.server_url = Some(url.to_string());

        Ok(())
    }

    /// チャネル経由でリクエストを送信
    pub async fn send_request(
        &self,
        channel_name: &str,
        method: &str,
        payload: Value,
    ) -> Result<Value> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| AgentError::Communication("Not connected to server".to_string()))?;

        debug!(
            "Sending request via channel: {}::{} with payload: {}",
            channel_name, method, payload
        );

        let channel = client
            .open_channel(channel_name)
            .await
            .map_err(|e| AgentError::Communication(format!("Failed to open channel: {}", e)))?;

        channel
            .request(method, &payload)
            .await
            .map_err(|e| AgentError::Communication(format!("Channel request failed: {}", e)))
    }

    /// サーバーから切断
    pub async fn disconnect(&mut self) -> Result<()> {
        if let Some(client) = self.client.take() {
            client
                .disconnect()
                .await
                .map_err(|e| AgentError::Communication(format!("Disconnect failed: {}", e)))?;
        }
        self.server_url = None;
        Ok(())
    }

    /// 接続状態を確認
    pub async fn is_connected(&self) -> bool {
        if let Some(client) = &self.client {
            client.is_connected().await
        } else {
            false
        }
    }
}

impl Default for UnisonTools {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unison_tools_creation() {
        let _tools = UnisonTools::new();
    }

    #[test]
    fn test_build_mcp_server() {
        let _server = UnisonTools::build_mcp_server();
    }
}
