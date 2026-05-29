//! Medium x Integration: mesh Custom trust の実 SAN 名前検証 E2E
//!
//! M3 (= `connect()` の SNI を "localhost" 固定から実ホスト/IP 導出に修正) の
//! **真の回帰テスト**であり、 Purple Haze 指摘「TrustAnchors::Custom (= 唯一の
//! secure production trust path) に E2E テストが無い」を解消する。
//!
//! 設計: `QuicServer::builder().cert_source(mesh_cert)` で mesh 発行の実 SAN 証明書を
//! 載せたサーバーを起動し、 `QuicClient::builder().trust_anchors(Custom)` でその CA を
//! 信頼するクライアントから接続する (= sociable test、 mock なしで TLS handshake →
//! identity → channel round-trip を実 QUIC で通す)。
//!
//! - `connects_when_san_matches`: cert SAN = 接続先 (`::1`) → handshake 成功 + round-trip。
//!   **M3 修正前は SNI が "localhost" 固定で SAN "::1" と不一致になり失敗していた**ので、
//!   このテストが通ること自体が SNI 修正の証拠。
//! - `rejected_when_san_mismatches`: cert SAN ≠ 接続先 → 名前検証で handshake 失敗
//!   (= SNI が実際に検証されている defense-in-depth 確認)。
//!
//! すべて `#[ignore]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tracing::Level;

use unison::network::cert::CertSource;
use unison::network::channel::UnisonChannel;
use unison::network::{
    InternalMeshKeypair, MessageType, ProtocolClient, ProtocolServer, QuicClient, QuicServer,
};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// mesh 証明書を載せた ping-pong サーバーを起動し、 shutdown sender と接続用アドレスを返す。
///
/// `QuicServer::builder().cert_source(...)` を使う点が高レベル `spawn_listen`
/// (= dev_localhost 固定) との違い。 accept ループ (identity + channel dispatch) は
/// `start_with_shutdown` でそのまま回す。
async fn spawn_mesh_server(cert: CertSource, bind: &str) -> Result<(oneshot::Sender<()>, String)> {
    let server = ProtocolServer::with_identity("mesh-e2e", "1.0.0", "test");
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

    let server = Arc::new(server);
    let mut quic = QuicServer::builder(Arc::clone(&server))
        .cert_source(cert)
        .build();
    quic.bind(bind)
        .await
        .map_err(|e| anyhow::anyhow!("bind failed: {e}"))?;
    let local = quic
        .local_addr()
        .ok_or_else(|| anyhow::anyhow!("server not bound"))?;
    let connect_str = format!("[{}]:{}", local.ip(), local.port());

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = quic.start_with_shutdown(shutdown_rx).await;
    });

    Ok((shutdown_tx, connect_str))
}

// ─────────────────────────────────────────────────
// positive: cert SAN が接続先と一致 → handshake 成功 + round-trip
// (= M3 SNI 修正前は "localhost" 固定で落ちていた)
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn connects_when_san_matches() -> Result<()> {
    init_tracing();

    // cert SAN = "::1" (= 接続先と一致する IP SAN)
    let pair = InternalMeshKeypair::generate(["::1".to_string()])?;
    let (shutdown, addr) = spawn_mesh_server(pair.server_cert_source, "[::1]:0").await?;

    let quic = QuicClient::builder()
        .trust_anchors(pair.client_trust_anchors)
        .build()?;
    let client = ProtocolClient::new(quic);

    // SNI = "::1" (M3 後)。 cert SAN "::1" と一致するので handshake 成功するはず。
    client.connect(&addr).await?;
    assert!(
        client.is_connected().await,
        "Custom trust + SAN 一致で接続成功すべき"
    );

    let channel = client.open_channel("ping-pong").await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("ping", &json!({ "message": "mesh" })),
    )
    .await??;
    assert_eq!(
        resp.get("message").and_then(|v| v.as_str()),
        Some("Pong: mesh")
    );

    client.disconnect().await?;
    let _ = shutdown.send(());
    Ok(())
}

// ─────────────────────────────────────────────────
// negative: cert SAN が接続先と不一致 → 名前検証で handshake 失敗
// (= SNI が実際に検証されている defense-in-depth 確認)
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn rejected_when_san_mismatches() -> Result<()> {
    init_tracing();

    // cert SAN = 接続先 (::1) とは無関係なホスト名
    let pair = InternalMeshKeypair::generate(["wrong.example.invalid".to_string()])?;
    let (shutdown, addr) = spawn_mesh_server(pair.server_cert_source, "[::1]:0").await?;

    let quic = QuicClient::builder()
        .trust_anchors(pair.client_trust_anchors)
        .build()?;
    let client = ProtocolClient::new(quic);

    // SNI = "::1" ≠ cert SAN "wrong.example.invalid" → 名前検証失敗で接続できないべき。
    let result = timeout(Duration::from_secs(5), client.connect(&addr)).await;
    let connect_failed = matches!(result, Ok(Err(_)) | Err(_));
    assert!(
        connect_failed,
        "SAN 不一致なら接続は失敗すべき (= SNI が実際に名前検証されている証拠)"
    );

    let _ = shutdown.send(());
    Ok(())
}
