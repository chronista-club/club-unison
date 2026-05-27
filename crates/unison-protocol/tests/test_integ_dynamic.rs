//! Large × E2E: `DynamicProtocol` + `SchemaRegistry` against a P1 discovery server
//!
//! Server に `enable_discovery` で KDL を載せて起動し、 client が
//! [`DynamicProtocol::fetch`] で schema を fetch、 validation 付きで request を
//! 送れることを E2E で確認する。
//!
//! Unison Hailing Epic α の P2-Rust deliverable の E2E 検証。
//! 設計: `spec/04-discovery/SPEC.md` §8。
//!
//! すべて `#[ignore = "Large: E2E test"]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::channel::UnisonChannel;
use unison::network::dynamic::{DynamicError, DynamicProtocol};
use unison::network::schema_registry::ValidationError;
use unison::network::{MessageType, ProtocolClient, ProtocolServer, ServerHandle};

/// テスト用 KDL — unison.discovery + test.echo (= 型検証対象)
const TEST_KDL: &str = r#"
protocol "test-dynamic" version="0.42.0" {
    namespace "test.dynamic.e2e"

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

/// テスト用 server を起動 (= discovery + echo handler 両方)
async fn start_dynamic_server() -> Result<(ServerHandle, String)> {
    let server = ProtocolServer::with_identity("test-dynamic-srv", "0.42.0", "test");
    server.enable_discovery(TEST_KDL).await?;

    // test.echo handler を別途登録 (= request "Ping" に対して Pong を返す)
    server
        .register_channel("test.echo", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request && msg.method == "Ping" => {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let msg_text = payload
                            .get("msg")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let count = payload.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
                        let reply = json!({
                            "reply": format!("Pong: {msg_text}"),
                            "count": count + 1,
                        });
                        if channel.send_response(msg.id, &msg.method, &reply).await.is_err() {
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

// ─────────────────────────────────────────────────────────────────────
// Test 1: fetch → registry build → metadata 一致
// ─────────────────────────────────────────────────────────────────────

/// E2E: DynamicProtocol::fetch が registry を build、 metadata が KDL と一致
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_dynamic_fetch_builds_registry() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_dynamic_server().await?;

    let client = Arc::new(ProtocolClient::new_default()?);
    client.connect(&addr).await?;

    let proto = timeout(
        Duration::from_secs(5),
        DynamicProtocol::fetch(client.clone()),
    )
    .await??;

    assert_eq!(proto.protocol_name(), "test-dynamic");
    assert_eq!(proto.version(), "0.42.0");
    assert_eq!(proto.hash().len(), 64);
    assert_eq!(proto.codecs(), &["json".to_string()][..]);

    // registry に test.echo + unison.discovery 両方ある
    let reg = proto.registry();
    assert!(reg.channel("test.echo").is_some());
    assert!(reg.channel("unison.discovery").is_some());
    assert_eq!(reg.channels().count(), 2);
    let req = reg.request("test.echo", "Ping").expect("Ping exists");
    assert_eq!(req.name, "Ping");

    info!(
        "DynamicProtocol fetched: {} v{} ({} channels)",
        proto.protocol_name(),
        proto.version(),
        reg.channels().count()
    );

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 2: valid request → server まで届く
// ─────────────────────────────────────────────────────────────────────

/// E2E: validation を通過した request が server に届き response が返る
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_dynamic_valid_request_round_trip() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_dynamic_server().await?;

    let client = Arc::new(ProtocolClient::new_default()?);
    client.connect(&addr).await?;
    let proto = DynamicProtocol::fetch(client.clone()).await?;

    let chan = proto.open_channel("test.echo").await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        chan.request("Ping", json!({"msg": "hello", "count": 0})),
    )
    .await??;

    assert_eq!(resp.get("reply").and_then(|v| v.as_str()), Some("Pong: hello"));
    assert_eq!(resp.get("count").and_then(|v| v.as_i64()), Some(1));

    chan.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 3: invalid request → ValidationError、 server には届かない (fail-fast)
// ─────────────────────────────────────────────────────────────────────

/// E2E: missing required field → ValidationError、 server に到達しない
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_dynamic_missing_required_is_fail_fast() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_dynamic_server().await?;

    let client = Arc::new(ProtocolClient::new_default()?);
    client.connect(&addr).await?;
    let proto = DynamicProtocol::fetch(client.clone()).await?;

    let chan = proto.open_channel("test.echo").await?;
    // msg field を欠く (= required)
    let err = chan
        .request("Ping", json!({"count": 1}))
        .await
        .unwrap_err();
    match err {
        DynamicError::Validation(ValidationError::MissingRequired { field, .. }) => {
            assert_eq!(field, "msg");
        }
        other => panic!("expected MissingRequired, got {other:?}"),
    }

    chan.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// E2E: type mismatch → ValidationError、 server に到達しない
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_dynamic_type_mismatch_is_fail_fast() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_dynamic_server().await?;

    let client = Arc::new(ProtocolClient::new_default()?);
    client.connect(&addr).await?;
    let proto = DynamicProtocol::fetch(client.clone()).await?;

    let chan = proto.open_channel("test.echo").await?;
    // count に string を渡す (= int expected)
    let err = chan
        .request("Ping", json!({"msg": "hi", "count": "not an int"}))
        .await
        .unwrap_err();
    match err {
        DynamicError::Validation(ValidationError::TypeMismatch {
            field,
            expected,
            got,
            ..
        }) => {
            assert_eq!(field, "count");
            assert_eq!(expected, "int");
            assert_eq!(got, "string");
        }
        other => panic!("expected TypeMismatch, got {other:?}"),
    }

    chan.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// E2E: unknown method → ValidationError、 server に到達しない
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_dynamic_unknown_method_is_fail_fast() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_dynamic_server().await?;

    let client = Arc::new(ProtocolClient::new_default()?);
    client.connect(&addr).await?;
    let proto = DynamicProtocol::fetch(client.clone()).await?;

    let chan = proto.open_channel("test.echo").await?;
    let err = chan
        .request("NotARealMethod", json!({}))
        .await
        .unwrap_err();
    match err {
        DynamicError::Validation(ValidationError::MethodNotFound { method, .. }) => {
            assert_eq!(method, "NotARealMethod");
        }
        other => panic!("expected MethodNotFound, got {other:?}"),
    }

    chan.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// E2E: open_channel に unknown channel → ValidationError、 server に到達しない
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_dynamic_unknown_channel_is_fail_fast() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_dynamic_server().await?;

    let client = Arc::new(ProtocolClient::new_default()?);
    client.connect(&addr).await?;
    let proto = DynamicProtocol::fetch(client.clone()).await?;

    match proto.open_channel("ghost.channel").await {
        Err(DynamicError::Validation(ValidationError::ChannelNotFound(name))) => {
            assert_eq!(name, "ghost.channel");
        }
        Ok(_) => panic!("expected ChannelNotFound error, got Ok"),
        Err(other) => panic!("expected ChannelNotFound, got {other:?}"),
    }

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
