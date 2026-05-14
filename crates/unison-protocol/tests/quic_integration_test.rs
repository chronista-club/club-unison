//! Large x E2E: QUIC プロトコル統合テスト
//!
//! 実際の QUIC サーバー/クライアント間で完全なプロトコルフローを検証する。
//! スキーマ読み込み → サーバー起動 → クライアント接続 → Identity ハンドシェイク
//! → チャネル通信 → 切断 → シャットダウン。
//!
//! すべて `#[ignore = "Large: E2E test"]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tracing::{Level, info};

use club_unison::network::MessageType;
use club_unison::network::channel::UnisonChannel;
use club_unison::{ProtocolClient, ProtocolServer, ServerHandle};

/// テスト用のトレーシング初期化（複数テストで呼ばれても安全）
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// E2E テスト用サーバーを起動して ServerHandle + アドレス文字列を返す
async fn start_e2e_server() -> Result<(ServerHandle, String)> {
    let server = ProtocolServer::with_identity("e2e-test", "1.0.0", "test");
    let start_time = Instant::now();

    // ping-pong チャネル: ping / echo / health メソッドを処理
    server
        .register_channel("ping-pong", move |_ctx, stream| async move {
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
                let request_id = msg.id;
                let method = msg.method.clone();

                let response = match method.as_str() {
                    "ping" => {
                        let message = payload
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Hello!");
                        let sequence = payload
                            .get("sequence")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        json!({
                            "message": format!("Pong: {}", message),
                            "sequence": sequence,
                        })
                    }
                    "echo" => {
                        let data = payload.get("data").cloned().unwrap_or_default();
                        let transform = payload
                            .get("transform")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let echoed_data = match transform {
                            "uppercase" if data.is_string() => {
                                json!(data.as_str().unwrap().to_uppercase())
                            }
                            "reverse" if data.is_string() => {
                                json!(data.as_str().unwrap().chars().rev().collect::<String>())
                            }
                            _ => data,
                        };
                        json!({ "echoed_data": echoed_data })
                    }
                    "health" => {
                        json!({
                            "status": "ok",
                            "uptime_ms": start_time.elapsed().as_millis() as u64,
                        })
                    }
                    _ => json!({"error": format!("unknown method: {}", method)}),
                };

                if channel
                    .send_response(request_id, &method, &response)
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
    let addr = handle.local_addr();
    let addr_str = format!("[{}]:{}", addr.ip(), addr.port());
    info!("E2E server started on {}", addr_str);

    Ok((handle, addr_str))
}

// ─────────────────────────────────────────────────
// Test 1: 完全なプロトコルフロー
// ─────────────────────────────────────────────────

/// スキーマ読み込み → サーバー → クライアント → チャネル → request/response → close
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_full_protocol_flow() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_e2e_server().await?;

    // クライアント接続（Identity Handshake 含む）
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    assert!(client.is_connected().await);

    // Identity 検証
    let identity = client
        .server_identity()
        .await
        .expect("Identity should exist");
    assert_eq!(identity.name, "e2e-test");
    assert_eq!(identity.version, "1.0.0");
    assert!(
        identity.channels.iter().any(|ch| ch.name == "ping-pong"),
        "ping-pong channel should be in identity"
    );
    info!("Identity verified: {} v{}", identity.name, identity.version);

    // チャネル開設 + Ping
    let channel = client.open_channel("ping-pong").await?;
    let response = timeout(
        Duration::from_secs(5),
        channel.request::<_, Value>("ping", &json!({"message": "E2E", "sequence": 1})),
    )
    .await??;
    assert_eq!(
        response.get("message").and_then(|v| v.as_str()),
        Some("Pong: E2E")
    );
    assert_eq!(response.get("sequence").and_then(|v| v.as_i64()), Some(1));
    info!("Ping-pong verified");

    // クリーンアップ
    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    info!("Full protocol flow completed");
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 2: Echo with data transformations
// ─────────────────────────────────────────────────

/// Echo ハンドラーの各種変換を E2E で検証
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_echo_transformations() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_e2e_server().await?;
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    let channel = client.open_channel("ping-pong").await?;

    // Uppercase
    let resp = timeout(
        Duration::from_secs(5),
        channel.request::<_, Value>(
            "echo",
            &json!({"data": "hello world", "transform": "uppercase"}),
        ),
    )
    .await??;
    assert_eq!(
        resp.get("echoed_data").and_then(|v| v.as_str()),
        Some("HELLO WORLD")
    );

    // Reverse
    let resp = timeout(
        Duration::from_secs(5),
        channel.request::<_, Value>("echo", &json!({"data": "abcd", "transform": "reverse"})),
    )
    .await??;
    assert_eq!(
        resp.get("echoed_data").and_then(|v| v.as_str()),
        Some("dcba")
    );

    // No transform (identity)
    let resp = timeout(
        Duration::from_secs(5),
        channel.request::<_, Value>("echo", &json!({"data": "unchanged", "transform": ""})),
    )
    .await??;
    assert_eq!(
        resp.get("echoed_data").and_then(|v| v.as_str()),
        Some("unchanged")
    );

    info!("Echo transformations verified");

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 3: Health チェック + サーバー uptime
// ─────────────────────────────────────────────────

/// Health メソッドでサーバー稼働状態を確認
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_health_check() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_e2e_server().await?;
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    let channel = client.open_channel("ping-pong").await?;

    let resp = timeout(Duration::from_secs(5), channel.request::<_, Value>("health", &json!({}))).await??;
    assert_eq!(resp.get("status").and_then(|v| v.as_str()), Some("ok"));
    assert!(
        resp.get("uptime_ms").and_then(|v| v.as_u64()).is_some(),
        "uptime_ms should be present"
    );
    info!("Health check: {:?}", resp);

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 4: 複雑な JSON ペイロードの往復
// ─────────────────────────────────────────────────

/// ネスト、配列、Unicode を含む複雑な JSON の E2E 往復
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_complex_json_roundtrip() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_e2e_server().await?;
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    let channel = client.open_channel("ping-pong").await?;

    let complex_data = json!({
        "nested": {
            "array": [1, 2, 3],
            "string": "テスト",
            "boolean": true,
            "null_val": null
        },
        "number": 42,
        "emoji": "🎵"
    });

    let resp = timeout(
        Duration::from_secs(5),
        channel.request::<_, Value>(
            "echo",
            &json!({"data": complex_data.clone(), "transform": ""}),
        ),
    )
    .await??;

    let echoed = resp.get("echoed_data").expect("echoed_data should exist");
    assert_eq!(echoed, &complex_data);
    info!("Complex JSON roundtrip verified");

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 5: 連続リクエストのスループット
// ─────────────────────────────────────────────────

/// 50 件の連続 ping リクエストを E2E で実行し、レイテンシを計測
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_sequential_throughput() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_e2e_server().await?;
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    let channel = client.open_channel("ping-pong").await?;

    let count = 50;
    let start = Instant::now();

    for i in 0..count {
        let resp = timeout(
            Duration::from_secs(5),
            channel.request::<_, Value>("ping", &json!({"message": "throughput", "sequence": i})),
        )
        .await??;
        assert_eq!(resp.get("sequence").and_then(|v| v.as_i64()), Some(i),);
    }

    let elapsed = start.elapsed();
    let avg_ms = elapsed.as_millis() as f64 / count as f64;
    info!(
        "{} requests in {:?} (avg {:.2} ms/req)",
        count, elapsed, avg_ms
    );

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 6: グレースフルシャットダウン中の通信
// ─────────────────────────────────────────────────

/// アクティブなチャネル通信中にサーバーをシャットダウン → クライアントが検知
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_graceful_shutdown() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_e2e_server().await?;
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    assert!(client.is_connected().await);

    // 通信が成功することを確認
    let channel = client.open_channel("ping-pong").await?;
    let resp = timeout(
        Duration::from_secs(5),
        channel.request::<_, Value>("ping", &json!({"message": "before shutdown", "sequence": 0})),
    )
    .await??;
    assert_eq!(
        resp.get("message").and_then(|v| v.as_str()),
        Some("Pong: before shutdown")
    );

    // サーバーシャットダウン
    handle.shutdown().await?;
    info!("Server shut down");

    // クライアントが切断を検知（ポーリングで最大 3 秒待機）
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if !client.is_connected().await {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("Client did not detect server shutdown within 3s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    info!("Client detected shutdown");

    client.disconnect().await?;
    Ok(())
}
