//! Medium x Integration: 型付きの tool が、 server の返事を agent から隠さない
//!
//! - handler が request を断った (error frame) → 理由を tool の結果 (isError) で返す
//! - 返事が `returns` の形と違う → 中身を text で返す (= MCP client の output schema 検証に
//!   落とされて、 中身が agent に届かなくなるのを防ぐ)
//! - 任意の field が null → 成功の形として通す
//!
//! すべて `#[ignore]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use rmcp::model::{CallToolResult, ContentBlock};
use unison::ProtocolServer;
use unison::network::server::ServerHandle;
use unison::network::{MessageType, UnisonChannel};
use unison_mcp::bridge::UnisonBridge;
use unison_mcp::config::{BridgeConfig, TrustMode};
use unison_mcp::mapping;
use unison_mcp::tools::UnisonMcp;

const KDL: &str = r#"
protocol "test-agent-errors" version="0.1.0" {
    namespace "test.agent.errors"

    channel "unison.discovery" from="client" lifetime="persistent" {
        request "GetProtocol" {
            field "format" type="string" required=#true
            returns "ProtocolDocument" {
                field "kdl" type="string" required=#true
                field "version" type="string" required=#true
                field "hash" type="string" required=#true
                field "codecs" type="json" required=#true
            }
        }
    }

    channel "sessions" from="client" lifetime="persistent" {
        request "Remove" {
            field "session" type="string" required=#true
            returns "Removed" {
                field "status" type="string" required=#true
                field "lane" type="string"
            }
        }
    }
}
"#;

/// `session` の値で返し方を変える server:
/// - `"root"` → error frame で断る
/// - `"legacy"` → 成功の frame に `{error: ...}` を詰める (= returns の形と違う)
/// - `"detached"` → 任意の `lane` を null で返す
/// - それ以外 → returns どおり
async fn start_server() -> Result<(ServerHandle, String)> {
    let server = ProtocolServer::with_identity("test-agent-errors", "0.1.0", "test");
    server.enable_discovery(KDL).await?;
    server
        .register_channel("sessions", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request => {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let session = payload.get("session").and_then(Value::as_str).unwrap_or("");
                        match session {
                            "root" => {
                                channel
                                    .send_error(
                                        msg.id,
                                        &msg.method,
                                        &json!({ "error": "root session は取り除けません" }),
                                    )
                                    .await?
                            }
                            "legacy" => {
                                channel
                                    .send_response(
                                        msg.id,
                                        &msg.method,
                                        &json!({ "error": "repo dispatch 失敗: 断られました" }),
                                    )
                                    .await?
                            }
                            "detached" => {
                                channel
                                    .send_response(
                                        msg.id,
                                        &msg.method,
                                        &json!({ "status": "removed", "lane": null }),
                                    )
                                    .await?
                            }
                            _ => {
                                channel
                                    .send_response(
                                        msg.id,
                                        &msg.method,
                                        &json!({ "status": "removed", "lane": "lead" }),
                                    )
                                    .await?
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
        })
        .await;
    let handle = Arc::new(server).listener("[::1]:0").spawn().await?;
    let addr = handle.local_addr();
    Ok((handle, format!("[{}]:{}", addr.ip(), addr.port())))
}

async fn start_mcp(endpoint: &str) -> Result<UnisonMcp> {
    let config = BridgeConfig {
        endpoint: Some(format!("quic://{endpoint}")),
        trust: Some(TrustMode::Skip),
    };
    Ok(UnisonMcp::new(UnisonBridge::new(config).await?))
}

async fn remove(mcp: &UnisonMcp, session: &str) -> Result<CallToolResult> {
    let tool = mapping::synth_tool_name("sessions", "Remove");
    Ok(timeout(
        Duration::from_secs(5),
        mcp.invoke_tool(&tool, json!({ "session": session })),
    )
    .await??)
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// handler が断った理由は、 tool の結果 (isError) の文面として agent に届く。
#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn refused_request_reports_the_reason_as_tool_error() -> Result<()> {
    let (handle, addr) = start_server().await?;
    let mcp = start_mcp(&addr).await?;

    let result = remove(&mcp, "root").await?;
    assert_eq!(result.is_error, Some(true));
    assert!(result.structured_content.is_none());
    let text = text_of(&result);
    assert!(text.contains("root session は取り除けません"), "{text}");

    handle.shutdown().await?;
    Ok(())
}

/// 返事が returns の形と違っても、 中身は text で agent に届く。 structuredContent には
/// 載せない (= 載せると MCP client の output schema 検証が落とし、 中身が消える)。
#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn response_that_does_not_match_returns_is_still_delivered() -> Result<()> {
    let (handle, addr) = start_server().await?;
    let mcp = start_mcp(&addr).await?;

    let result = remove(&mcp, "legacy").await?;
    assert_eq!(result.is_error, Some(true));
    assert!(result.structured_content.is_none());
    let text = text_of(&result);
    assert!(text.contains("repo dispatch 失敗: 断られました"), "{text}");
    assert!(
        text.contains("status"),
        "足りない field を名指しするべき: {text}"
    );

    handle.shutdown().await?;
    Ok(())
}

/// 任意の field が null の返事は、 成功の形として通る (= Rust の `Option` は null になる)。
#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn optional_field_may_be_null() -> Result<()> {
    let (handle, addr) = start_server().await?;
    let mcp = start_mcp(&addr).await?;

    let result = remove(&mcp, "detached").await?;
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert_eq!(
        result.structured_content,
        Some(json!({ "status": "removed", "lane": null }))
    );

    // 配る output schema も、 任意の field に null を許す (= client の検証を通る)。
    let tool = mcp
        .find_tool(&mapping::synth_tool_name("sessions", "Remove"))
        .expect("synthesized tool");
    let schema = Value::Object((*tool.output_schema.expect("output schema")).clone());
    assert_eq!(
        schema["properties"]["lane"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(schema["properties"]["status"]["type"], json!("string"));

    // returns どおりの返事はこれまでどおり structuredContent で返る。
    let ok = remove(&mcp, "s1").await?;
    assert_eq!(
        ok.structured_content,
        Some(json!({ "status": "removed", "lane": "lead" }))
    );

    handle.shutdown().await?;
    Ok(())
}
