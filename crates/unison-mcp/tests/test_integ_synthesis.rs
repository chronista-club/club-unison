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

/// refresh E2E 用の「進化した」schema — 別 channel / 別 request / 別 version
/// (= TEST_KDL と protocol hash が必ず異なる)
const TEST_KDL_V2: &str = r#"
protocol "test-synth" version="0.43.0" {
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

    channel "test.probe" from="client" lifetime="persistent" {
        request "Probe" {
            field "q" type="string" required=#true
            returns "Ack" {
                field "ack" type="string" required=#true
            }
        }
    }
}
"#;

/// 指定 addr に TEST_KDL_V2 の server を起動する (= schema 進化後の server を模擬)。
/// `spawn_listen` は self を consume するため、 bind retry は server ごと作り直す
/// (= 直前の server の UDP socket close 直後は bind が失敗し得る)。
async fn start_test_server_v2(addr: &str) -> Result<ServerHandle> {
    let mut last_err: Option<anyhow::Error> = None;
    for _ in 0..10 {
        let server = ProtocolServer::with_identity("test-synth-srv2", "0.43.0", "test");
        server.enable_discovery(TEST_KDL_V2).await?;
        server
            .register_channel("test.probe", |_ctx, stream| async move {
                let channel = UnisonChannel::new(stream);
                loop {
                    match channel.recv().await {
                        Ok(msg)
                            if msg.msg_type == MessageType::Request && msg.method == "Probe" =>
                        {
                            let payload = msg.payload_as_value().unwrap_or_default();
                            let q = payload.get("q").and_then(|v| v.as_str()).unwrap_or("");
                            let reply = json!({ "ack": format!("ack: {q}") });
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
        match server.spawn_listen(addr).await {
            Ok(h) => return Ok(h),
            Err(e) => {
                last_err = Some(e.into());
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    Err(last_err.expect("at least one bind attempt"))
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

    // title は人間可読な channel.method (= rmcp 2.x)
    assert_eq!(echo_tool.title.as_deref(), Some("test.echo.Ping"));

    // KDL `returns` block から output_schema が合成される (= rmcp 2.x)
    let out_schema: Value = serde_json::to_value(
        echo_tool
            .output_schema
            .as_ref()
            .expect("returns block must produce output_schema")
            .as_ref(),
    )?;
    assert_eq!(out_schema.get("type"), Some(&json!("object")));
    let out_props = out_schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("output properties");
    assert!(out_props.contains_key("reply"));
    assert!(out_props.contains_key("count"));
    let out_required = out_schema
        .get("required")
        .and_then(Value::as_array)
        .expect("output required");
    assert!(out_required.contains(&json!("reply")));
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

    // structured_content = response そのもの (= output_schema と同形、 rmcp 2.x)
    let resp = result
        .structured_content
        .as_ref()
        .expect("structured content present");
    assert_eq!(
        resp.get("reply").and_then(Value::as_str),
        Some("Pong: hello")
    );
    assert_eq!(resp.get("count").and_then(Value::as_i64), Some(1));

    // text content は structured の互換 mirror (= 同じ JSON にパースできる)
    let content_text = match result.content.first() {
        Some(c) => match c {
            rmcp::model::ContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text content"),
        },
        None => panic!("no content in CallToolResult"),
    };
    let parsed: Value = serde_json::from_str(&content_text)?;
    assert_eq!(&parsed, resp, "text mirror must match structured content");

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

// ─────────────────────────────────────────────────────────────────────
// Test 5: default endpoint への unison_discover が synthesized tool set を
// refresh する (= live bridge、 server の schema 進化に session 中に追従)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_discover_refreshes_synthesized_tools() -> Result<()> {
    init_tracing();
    let (h1, addr) = start_test_server().await?;
    let mcp = start_mcp_with_endpoint(&addr).await?;

    let ping_tool = mapping::synth_tool_name("test.echo", "Ping");
    let probe_tool = mapping::synth_tool_name("test.probe", "Probe");
    let names: Vec<String> = mcp.all_tools().iter().map(|t| t.name.to_string()).collect();
    assert!(names.contains(&ping_tool), "v1 tool present: {names:?}");
    assert!(!names.contains(&probe_tool), "v2 tool absent: {names:?}");

    // server の schema 進化を模擬: 同一 endpoint で異なる KDL の server に入れ替える
    h1.shutdown().await?;
    let h2 = start_test_server_v2(&addr).await?;

    // default endpoint (= endpoint arg 省略) への discover が refresh を起こす
    let result = timeout(
        Duration::from_secs(10),
        mcp.invoke_tool("unison_discover", json!({})),
    )
    .await??;
    let sc = result
        .structured_content
        .as_ref()
        .expect("structured content");
    assert_eq!(sc.get("refreshed"), Some(&json!(true)));
    assert_eq!(
        sc.get("version").and_then(Value::as_str),
        Some("0.43.0"),
        "summary must reflect the new server schema"
    );

    // synthesized tool set が新 schema に入れ替わっている
    let names: Vec<String> = mcp.all_tools().iter().map(|t| t.name.to_string()).collect();
    assert!(
        names.contains(&probe_tool),
        "new schema tool must appear: {names:?}"
    );
    assert!(
        !names.contains(&ping_tool),
        "old schema tool must disappear: {names:?}"
    );

    // 入れ替わった tool が実際に dispatch できる (= 新 connection 経由の round-trip)
    let result = timeout(
        Duration::from_secs(5),
        mcp.invoke_tool(&probe_tool, json!({ "q": "hello" })),
    )
    .await??;
    let sc = result.structured_content.expect("probe structured");
    assert_eq!(sc.get("ack").and_then(Value::as_str), Some("ack: hello"));

    h2.shutdown().await?;
    Ok(())
}
