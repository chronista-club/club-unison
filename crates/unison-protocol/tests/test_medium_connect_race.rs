//! Medium x Integration: `connect_race`（Happy Eyeballs v2 dialer、ADR-020 §S6）e2e。
//!
//! engine の並行タイミングは `network::dial` の unit test が仮想時間で押さえている。
//! ここは **実 quinn 相手** に「複数 direct 候補の staggered race が、死んだ decoy を
//! 飛ばして生きている world へ実際に QUIC 接続を確立する」ことを round-trip で担保する。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行。

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::MessageType;
use unison::network::RaceCfg;
use unison::network::channel::UnisonChannel;
use unison::{ProtocolClient, ProtocolServer, ServerHandle};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// `[::1]` に ping-pong エコーサーバーを起動し、`ServerHandle` と実 bind アドレスを返す。
async fn start_echo_server() -> Result<(ServerHandle, SocketAddr)> {
    let server = ProtocolServer::with_identity("connect-race-test", "1.0.0", "test");

    server
        .register_channel("ping-pong", |_ctx, stream| async move {
            let channel: UnisonChannel = UnisonChannel::new(stream);
            loop {
                let msg = match channel.recv().await {
                    Ok(msg) => msg,
                    Err(_) => break,
                };
                if msg.msg_type != MessageType::Request {
                    continue;
                }
                let payload = msg.payload_as_value().unwrap_or_default();
                let message = payload
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Hello!");
                let response = json!({ "message": format!("Pong: {}", message) });
                if channel
                    .send_response(msg.id, &msg.method, &response)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(())
        })
        .await;

    let handle = server.spawn_listen("[::1]:0").await?;
    let local = handle.local_addr();
    info!("connect-race echo server listening on {local}");
    Ok((handle, local))
}

/// 接続済みクライアントで ping-pong を 1 往復して応答を検証する。
async fn assert_ping_pong(client: &ProtocolClient) -> Result<()> {
    let channel = client.open_channel("ping-pong").await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("ping", &json!({ "message": "race" })),
    )
    .await??;
    assert_eq!(
        resp.get("message").and_then(|v| v.as_str()),
        Some("Pong: race"),
        "ping-pong round-trip should echo via the raced connection"
    );
    Ok(())
}

/// テスト用に stagger を詰めた RaceCfg。
fn fast_cfg() -> RaceCfg {
    RaceCfg {
        stagger: Duration::from_millis(100),
        relay_handicap: Duration::from_millis(200),
        overall_deadline: Duration::from_secs(5),
    }
}

// ─────────────────────────────────────────────────
// Test 1: 死んだ decoy を先頭に置いても、生きている world を race で掴む
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn connect_race_prefers_live_over_dead_decoy() -> Result<()> {
    init_tracing();

    let (handle, live) = start_echo_server().await?;
    // decoy = loopback port 1（誰も listen していない）を先頭候補に。
    let dead: SocketAddr = "[::1]:1".parse().unwrap();

    let client = ProtocolClient::new_default()?;
    client
        .connect_race(vec![dead, live], "::1", fast_cfg())
        .await?;
    assert!(client.is_connected().await, "race should establish a live connection");
    // decoy には server が居ないため、ping-pong が成立すること自体が「race が live 経路を
    // 掴んだ」証明（dead を掴んでいたら open_channel/request が timeout する）。
    assert_ping_pong(&client).await?;

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 2: 単一 live 候補（sanity — 単発 direct と等価）
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn connect_race_single_live_candidate() -> Result<()> {
    init_tracing();

    let (handle, live) = start_echo_server().await?;

    let client = ProtocolClient::new_default()?;
    client.connect_race(vec![live], "::1", fast_cfg()).await?;
    assert!(client.is_connected().await);

    assert_ping_pong(&client).await?;

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
