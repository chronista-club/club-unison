//! Large × E2E: unison-mcp が discovery server から synthesized typed tools を
//! 動的に exposure し、 invoke 時に DynamicChannel 経由で validation + dispatch
//! することを検証する。
//!
//! Unison Hailing α Epic P3b の E2E。
//!
//! すべて `#[ignore = "Large: E2E test"]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use rmcp::ErrorData as McpError;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::channel::UnisonChannel;
use unison::network::{MessageType, ProtocolServer, ServerHandle};
use unison_mcp::bridge::UnisonBridge;
use unison_mcp::config::{BridgeConfig, TrustMode};
use unison_mcp::mapping;
use unison_mcp::tools::UnisonMcp;

/// テスト用 KDL — discovery + test.echo (= 型検証対象)
const TEST_KDL: &str = r#"
protocol "test-synth" version="0.42.0" {
    namespace "test.synth.e2e"

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

    channel "test.echo" from="client" lifetime="persistent" {
        request "Ping" {
            field "msg" type="string" required=#true
            field "count" type="int"
            returns "Pong" {
                field "reply" type="string" required=#true
                field "count" type="int"
            }
        }
    }
}
"#;

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// discovery + echo handler 付き server を起動
async fn start_test_server() -> Result<(ServerHandle, String)> {
    let server = ProtocolServer::with_identity("test-synth-srv", "0.42.0", "test");
    server.enable_discovery(TEST_KDL).await?;
    server
        .register_channel("test.echo", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request && msg.method == "Ping" => {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let msg_text = payload.get("msg").and_then(|v| v.as_str()).unwrap_or("");
                        let count = payload.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
                        let reply = json!({
                            "reply": format!("Pong: {msg_text}"),
                            "count": count + 1,
                        });
                        if channel
                            .send_response(msg.id, &msg.method, &reply)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => continue,
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })
        .await;
    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();
    Ok((handle, format!("[{}]:{}", addr.ip(), addr.port())))
}

/// 接続済 server + UnisonMcp を組み立てて返す
async fn start_mcp_with_endpoint(endpoint: &str) -> Result<UnisonMcp> {
    let config = BridgeConfig {
        endpoint: Some(format!("quic://{endpoint}")),
        trust: Some(TrustMode::Skip),
    };
    let bridge = UnisonBridge::new(config).await?;
    Ok(UnisonMcp::new(bridge))
}

// ─────────────────────────────────────────────────────────────────────
// Test 1: bridge が eagerly fetch + UnisonMcp が synthesized tools を merged 列挙
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_synthesis_lists_static_plus_synthesized_tools() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_test_server().await?;
    let mcp = start_mcp_with_endpoint(&addr).await?;

    let tools = mcp.all_tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

    // static 3 (= ping/call/discover)
    assert!(
        names.contains(&"unison_ping"),
        "static unison_ping must be present: {names:?}"
    );
    assert!(names.contains(&"unison_call"));
    assert!(names.contains(&"unison_discover"));

    // synthesized = test-synth KDL の GetProtocol + Ping
    let synth_discovery = mapping::synth_tool_name("unison.discovery", "GetProtocol");
    let synth_ping = mapping::synth_tool_name("test.echo", "Ping");
    assert!(
        names.contains(&synth_discovery.as_str()),
        "synthesized {synth_discovery} must be present: {names:?}"
    );
    assert!(
        names.contains(&synth_ping.as_str()),
        "synthesized {synth_ping} must be present: {names:?}"
    );

    // synthesized tool の input_schema を validate
    let echo_tool = tools
        .iter()
        .find(|t| t.name.as_ref() == synth_ping)
        .expect("found echo tool");
    let schema_value: Value = serde_json::to_value(echo_tool.input_schema.as_ref())?;
    assert_eq!(schema_value.get("type"), Some(&json!("object")));
    let props = schema_value
        .get("properties")
        .and_then(Value::as_object)
        .expect("properties is object");
    assert!(props.contains_key("msg"));
    assert!(props.contains_key("count"));
    let required = schema_value
        .get("required")
        .and_then(Value::as_array)
        .expect("required present");
    assert!(required.contains(&json!("msg")));
    info!(
        "synthesized {} tools total ({} synthesized + 3 static)",
        tools.len(),
        tools.len() - 3
    );

    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 2: invoke synthesized tool → DynamicChannel 経由で実行 → response が返る
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_synthesis_invoke_synthesized_tool_round_trip() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_test_server().await?;
    let mcp = start_mcp_with_endpoint(&addr).await?;

    let tool = mapping::synth_tool_name("test.echo", "Ping");
    let result = timeout(
        Duration::from_secs(5),
        mcp.invoke_tool(&tool, json!({ "msg": "hello", "count": 0 })),
    )
    .await??;

    // CallToolResult の content[0] が text、 JSON parse して中身を assert
    let content_text = match result.content.first() {
        Some(c) => match &c.raw {
            rmcp::model::RawContent::Text(t) => t.text.clone(),
            _ => panic!("expected text content"),
        },
        None => panic!("no content in CallToolResult"),
    };
    let parsed: Value = serde_json::from_str(&content_text)?;
    let resp = parsed.get("response").expect("response field");
    assert_eq!(
        resp.get("reply").and_then(Value::as_str),
        Some("Pong: hello")
    );
    assert_eq!(resp.get("count").and_then(Value::as_i64), Some(1));

    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 3: invalid payload → ValidationError → MCP invalid_request、 server に届かない
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_synthesis_validation_error_fails_fast() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_test_server().await?;
    let mcp = start_mcp_with_endpoint(&addr).await?;

    let tool = mapping::synth_tool_name("test.echo", "Ping");
    // msg field を欠く (= required)
    let result = mcp.invoke_tool(&tool, json!({ "count": 1 })).await;
    assert_invalid_request(result, "msg")?;

    // count に string (= int expected) → TypeMismatch
    let result = mcp
        .invoke_tool(&tool, json!({ "msg": "hi", "count": "not-int" }))
        .await;
    assert_invalid_request(result, "count")?;

    handle.shutdown().await?;
    Ok(())
}

/// McpError が invalid_request で、 message に期待される field 名を含むことを検証
fn assert_invalid_request<T>(result: Result<T, McpError>, expected_field_in_msg: &str) -> Result<()>
where
    T: std::fmt::Debug,
{
    match result {
        Err(e) => {
            let msg = format!("{e:?}");
            // ErrorCode::INVALID_REQUEST or INVALID_PARAMS だが、 message 比較で十分
            assert!(
                msg.contains(expected_field_in_msg) || msg.contains("validation"),
                "expected validation error mentioning '{expected_field_in_msg}', got: {msg}"
            );
            Ok(())
        }
        Ok(other) => {
            anyhow::bail!("expected error, got: {other:?}");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Test 4: find_tool が synthesized + static の双方を返す
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_synthesis_find_tool_finds_synthesized() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_test_server().await?;
    let mcp = start_mcp_with_endpoint(&addr).await?;

    assert!(mcp.find_tool("unison_ping").is_some());
    let synth = mapping::synth_tool_name("test.echo", "Ping");
    assert!(
        mcp.find_tool(&synth).is_some(),
        "should find synthesized tool {synth}"
    );
    assert!(mcp.find_tool("ghost_tool").is_none());

    handle.shutdown().await?;
    Ok(())
}
