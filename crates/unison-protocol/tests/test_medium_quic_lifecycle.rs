//! Medium x Integration: QUIC ライフサイクルテスト
//!
//! 実際の QUIC サーバー/クライアントを起動し、接続・通信・切断の
//! ライフサイクルを検証する。
//!
//! すべて `#[ignore]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::MessageType;
use unison::network::channel::UnisonChannel;
use unison::{ProtocolClient, ProtocolServer};

/// テスト用のトレーシング初期化（複数テストで呼ばれても安全）
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// テスト用のエコーチャネルハンドラーを登録
async fn register_echo_handler(server: &ProtocolServer) {
    server
        .register_channel("echo", |_ctx, stream| async move {
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
                if channel
                    .send_response(msg.id, &msg.method, &payload)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(())
        })
        .await;
}

// ─────────────────────────────────────────────────
// Test 1: サーバー起動 → ポート割り当て → シャットダウン
// ─────────────────────────────────────────────────

/// spawn_listen でポート 0 に bind → local_addr() で割り当てポートを取得 → shutdown
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_quic_server_bind_and_shutdown() -> Result<()> {
    init_tracing();

    let server = ProtocolServer::new();
    let handle = server.spawn_listen("[::1]:0").await?;

    let addr = handle.local_addr();
    info!("Server bound to {}", addr);

    // ポートが割り当てられている（0 ではない）
    assert_ne!(addr.port(), 0);
    assert!(!handle.is_finished());

    // グレースフルシャットダウン
    handle.shutdown().await?;
    info!("Server shut down successfully");

    Ok(())
}

// ─────────────────────────────────────────────────
// Test 2: 接続ライフサイクル（connect → is_connected → disconnect）
// ─────────────────────────────────────────────────

/// クライアントがサーバーに接続し、接続状態を確認し、切断する
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_quic_connection_lifecycle() -> Result<()> {
    init_tracing();

    // サーバー起動
    let server = ProtocolServer::new();
    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();
    info!("Server listening on {}", addr);

    // クライアント接続
    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;
    info!("Client connected");

    // 接続状態の確認
    assert!(client.is_connected().await);

    // 切断
    client.disconnect().await?;
    info!("Client disconnected");

    // シャットダウン
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 3: Identity ハンドシェイク
// ─────────────────────────────────────────────────

/// 接続直後にサーバーから ServerIdentity を受信する
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_quic_identity_handshake() -> Result<()> {
    init_tracing();

    // サーバー起動（カスタム Identity）
    let server = ProtocolServer::with_identity("test-server", "0.1.0", "test-ns");
    register_echo_handler(&server).await;

    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();

    // クライアント接続（Identity Handshake は connect() 内で自動実行）
    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;

    // Identity が受信できていること
    let identity = client.server_identity().await;
    assert!(
        identity.is_some(),
        "Identity should be received after connect"
    );

    let identity = identity.unwrap();
    assert_eq!(identity.name, "test-server");
    assert_eq!(identity.version, "0.1.0");
    assert_eq!(identity.namespace, "test-ns");

    // 登録チャネルが Identity に反映されている
    assert!(
        identity.channels.iter().any(|ch| ch.name == "echo"),
        "Identity should include registered 'echo' channel"
    );
    info!("Identity handshake verified: {:?}", identity);

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 4: チャネル経由の Request/Response
// ─────────────────────────────────────────────────

/// open_channel → request → response の完全なフロー
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_quic_channel_request_response() -> Result<()> {
    init_tracing();

    // サーバー起動 + エコーハンドラー登録
    let server = ProtocolServer::new();
    register_echo_handler(&server).await;

    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();

    // クライアント接続
    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;

    // チャネル開設
    let channel = client.open_channel("echo").await?;

    // Request/Response
    let payload = serde_json::json!({"message": "hello", "number": 42});
    let response = timeout(
        Duration::from_secs(5),
        channel.request::<_, serde_json::Value>("echo", &payload),
    )
    .await??;

    assert_eq!(response, payload, "Echo should return same payload");
    info!("Request/Response verified: {:?}", response);

    // クリーンアップ
    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 5: 複数リクエストの連続送信
// ─────────────────────────────────────────────────

/// 同一チャネルで複数の Request/Response を連続実行
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_quic_sequential_requests() -> Result<()> {
    init_tracing();

    let server = ProtocolServer::new();
    register_echo_handler(&server).await;

    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;

    let channel = client.open_channel("echo").await?;

    // 10 件のリクエストを連続送信
    for i in 0..10 {
        let payload = serde_json::json!({"seq": i});
        let response = timeout(
            Duration::from_secs(5),
            channel.request::<_, serde_json::Value>("echo", &payload),
        )
        .await??;

        assert_eq!(
            response.get("seq").and_then(|v| v.as_i64()),
            Some(i),
            "Sequence {} should match",
            i
        );
    }
    info!("10 sequential requests completed successfully");

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 6: サーバーシャットダウン時のクライアント挙動
// ─────────────────────────────────────────────────

/// サーバーがシャットダウンした後、クライアントの接続状態が反映される
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_quic_server_shutdown_disconnects_client() -> Result<()> {
    init_tracing();

    let server = ProtocolServer::new();
    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;
    assert!(client.is_connected().await);

    // サーバーをシャットダウン
    handle.shutdown().await?;
    info!("Server shut down");

    // 少し待って接続断を検出させる
    tokio::time::sleep(Duration::from_millis(500)).await;

    // クライアントが切断を検知（QUIC の close_reason が設定される）
    assert!(
        !client.is_connected().await,
        "Client should detect server shutdown"
    );
    info!("Client correctly detected server shutdown");

    client.disconnect().await?;
    Ok(())
}
