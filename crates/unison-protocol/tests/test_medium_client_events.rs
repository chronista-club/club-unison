//! Medium x Integration: ProtocolClient connection event hook テスト (v0.10.0 Step 2)
//!
//! `ProtocolClient::subscribe_connection_events` が Connected / Disconnected event を
//! 実 QUIC connection lifecycle に同期して fire するかを検証する。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::ClientConnectionEvent;
use unison::{ProtocolClient, ProtocolServer};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// connect 成功時に `Connected { remote_addr }` event が fire される
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_client_event_connected_on_connect() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let server_addr = handle.local_addr();
    info!("Server bound to {}", server_addr);

    let client = ProtocolClient::new_default()?;
    let mut rx = client.subscribe_connection_events();

    // 接続 (= subscribe は接続 *前* に行ったので Connected を取れる)
    client
        .connect(&format!("[{}]:{}", server_addr.ip(), server_addr.port()))
        .await?;

    // Connected event を待つ
    let ev = timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("Connected event timeout")
        .expect("recv error");
    match ev {
        ClientConnectionEvent::Connected { remote_addr } => {
            assert_eq!(remote_addr, server_addr);
            info!("Got Connected event for {}", remote_addr);
        }
        other => panic!("expected Connected, got: {:?}", other),
    }

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// 明示 disconnect で `Disconnected { reason }` event が fire される
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_client_event_disconnected_on_explicit_disconnect() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let server_addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    let mut rx = client.subscribe_connection_events();
    client
        .connect(&format!("[{}]:{}", server_addr.ip(), server_addr.port()))
        .await?;

    // Connected 1 件を skip
    let _ = timeout(Duration::from_millis(500), rx.recv()).await;

    // 明示 disconnect
    client.disconnect().await?;

    // Disconnected event を待つ (= explicit disconnect の理由文字列 or drop detection
    // 由来の closed message、 どちらか到達すれば OK)
    let mut got_disconnected = false;
    for _ in 0..5 {
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(ClientConnectionEvent::Disconnected { reason })) => {
                info!("Got Disconnected: {}", reason);
                got_disconnected = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(
        got_disconnected,
        "expected Disconnected event after explicit disconnect"
    );

    handle.shutdown().await?;
    Ok(())
}

/// Server shutdown による drop detection で Disconnected event が fire される
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_client_event_disconnected_on_server_shutdown() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let server_addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    let mut rx = client.subscribe_connection_events();
    client
        .connect(&format!("[{}]:{}", server_addr.ip(), server_addr.port()))
        .await?;

    // Connected を 1 件 skip
    let _ = timeout(Duration::from_millis(500), rx.recv()).await;

    // Server を shutdown (= QUIC connection が close される、 drop detection task が発火)
    handle.shutdown().await?;

    // Disconnected を受信
    let mut got_disconnected = false;
    for _ in 0..10 {
        match timeout(Duration::from_millis(300), rx.recv()).await {
            Ok(Ok(ClientConnectionEvent::Disconnected { reason })) => {
                info!("Got Disconnected after server shutdown: {}", reason);
                got_disconnected = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(
        got_disconnected,
        "expected Disconnected event after server shutdown"
    );
    Ok(())
}

/// 複数 subscriber が同 event を独立に受け取れる (= broadcast 性質、 integration version)
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_client_event_multiple_subscribers() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let server_addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    let mut rx_a = client.subscribe_connection_events();
    let mut rx_b = client.subscribe_connection_events();

    client
        .connect(&format!("[{}]:{}", server_addr.ip(), server_addr.port()))
        .await?;

    // 両 subscriber が Connected を受信
    for (name, rx) in [("A", &mut rx_a), ("B", &mut rx_b)] {
        let ev = timeout(Duration::from_millis(500), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("subscriber {} timeout", name))
            .unwrap_or_else(|e| panic!("subscriber {} recv error: {}", name, e));
        assert!(
            matches!(ev, ClientConnectionEvent::Connected { .. }),
            "subscriber {} expected Connected, got: {:?}",
            name,
            ev
        );
    }

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
