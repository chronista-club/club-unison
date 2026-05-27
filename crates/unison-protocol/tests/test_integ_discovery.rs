//! Large × E2E: `unison.discovery` channel round-trip
//!
//! Server に [`ProtocolServer::enable_discovery`] で KDL を載せて起動し、
//! client が `unison.discovery` channel を open して `GetProtocol` request を
//! 投げ、 `ProtocolDocument` を受け取れることを実 QUIC stack 上で確認する。
//!
//! Unison Hailing Epic α の P1 deliverable の E2E 検証。
//! 設計: `spec/04-discovery/SPEC.md`
//!
//! すべて `#[ignore = "Large: E2E test"]` 付き — `cargo test -- --ignored`
//! で実行。

use anyhow::Result;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::channel::UnisonChannel;
use unison::network::discovery::{
    DISCOVERY_CHANNEL_NAME, GET_PROTOCOL_METHOD, ProtocolDocument,
};
use unison::{ProtocolClient, ProtocolServer, ServerHandle};

/// テスト用の最小 KDL (= unison-discovery 自身の channel と test.echo channel)
const TEST_KDL: &str = r#"
protocol "test-discovery" version="0.42.0" {
    namespace "test.discovery.e2e"

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
            field "msg" type="string"
            returns "Pong" {
                field "reply" type="string"
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

/// テスト用 server を起動して (handle, "[::1]:port") を返す
async fn start_discovery_server() -> Result<(ServerHandle, String)> {
    let server = ProtocolServer::with_identity("test-discovery-srv", "0.42.0", "test");
    server.enable_discovery(TEST_KDL).await?;
    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();
    Ok((handle, format!("[{}]:{}", addr.ip(), addr.port())))
}

/// Helper: Value response を ProtocolDocument にパース
fn parse_doc(value: Value) -> Result<ProtocolDocument> {
    serde_json::from_value(value).map_err(Into::into)
}

// ─────────────────────────────────────────────────────────────────────
// Test 1: GetProtocol round-trip
// ─────────────────────────────────────────────────────────────────────

/// E2E: enable_discovery → client.open_channel("unison.discovery")
/// → GetProtocol → ProtocolDocument を受け取る
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_discovery_get_protocol_round_trip() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_discovery_server().await?;

    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    assert!(client.is_connected().await);

    // ServerIdentity に unison.discovery が含まれる
    let identity = client.server_identity().await.expect("identity exists");
    assert!(
        identity
            .channels
            .iter()
            .any(|c| c.name == DISCOVERY_CHANNEL_NAME),
        "Identity should advertise unison.discovery channel"
    );

    // discovery channel を open
    let channel: UnisonChannel = client.open_channel(DISCOVERY_CHANNEL_NAME).await?;

    // GetProtocol request 送信、 Value で受信して ProtocolDocument に decode
    let value: Value = timeout(
        Duration::from_secs(5),
        channel.request(GET_PROTOCOL_METHOD, &json!({ "format": "kdl+hash" })),
    )
    .await??;
    let doc = parse_doc(value)?;

    // ProtocolDocument の中身を validate
    assert_eq!(doc.version, "0.42.0");
    assert!(
        doc.kdl.contains("protocol \"test-discovery\""),
        "kdl should contain protocol header"
    );
    assert!(
        doc.kdl.contains("test.echo"),
        "kdl should include other declared channels too"
    );
    assert_eq!(doc.hash.len(), 64);
    assert!(
        doc.hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "hash should be lowercase hex: {}",
        doc.hash
    );
    assert_eq!(doc.codecs, vec!["json".to_string()]);
    info!(
        "Discovery round-trip OK: version={} hash={}",
        doc.version,
        &doc.hash[..16]
    );

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 2: hash is deterministic across multiple calls
// ─────────────────────────────────────────────────────────────────────

/// E2E: 同じ server に対して GetProtocol を 2 度叩くと同じ hash / kdl / version を返す。
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_discovery_hash_is_deterministic() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_discovery_server().await?;

    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    let channel = client.open_channel(DISCOVERY_CHANNEL_NAME).await?;

    let v1: Value = timeout(
        Duration::from_secs(5),
        channel.request(GET_PROTOCOL_METHOD, &json!({ "format": "kdl" })),
    )
    .await??;
    let v2: Value = timeout(
        Duration::from_secs(5),
        channel.request(GET_PROTOCOL_METHOD, &json!({ "format": "kdl" })),
    )
    .await??;
    let d1 = parse_doc(v1)?;
    let d2 = parse_doc(v2)?;

    assert_eq!(d1.hash, d2.hash);
    assert_eq!(d1.kdl, d2.kdl);
    assert_eq!(d1.version, d2.version);
    assert_eq!(d1.codecs, d2.codecs);

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 3: ServerIdentity advertises unison.discovery when enabled
// ─────────────────────────────────────────────────────────────────────

/// E2E: enable_discovery を呼ぶと ServerIdentity.channels に
/// unison.discovery が追加される (= client は接続直後に detect 可能)。
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_discovery_appears_in_server_identity() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_discovery_server().await?;

    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;

    let identity = client.server_identity().await.expect("identity exists");
    assert_eq!(identity.name, "test-discovery-srv");
    assert_eq!(identity.version, "0.42.0");
    let names: Vec<&str> = identity.channels.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&DISCOVERY_CHANNEL_NAME),
        "ServerIdentity should advertise unison.discovery, got: {names:?}"
    );

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
