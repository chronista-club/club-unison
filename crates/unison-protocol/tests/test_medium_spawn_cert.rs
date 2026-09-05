//! Medium x Integration: 高レベル spawn 経路が cert を honor することの E2E
//!
//! chronista-hub handoff の回帰テスト。 かつての `spawn_listen` / `spawn_listen_shared`
//! は内部で `QuicServer::builder().build()` (= `dev_localhost` 固定) をハードコード
//! しており、 `cert_source()` をバイパスしていた。 結果 spawn 経路で立てたサーバーは
//! dev_localhost cert しか出せず、 非 loopback 公開 (tailnet / public federation) が
//! できなかった。
//!
//! v1.2.0 で cert を渡せる spawn 経路が入り、 2.0.0 でその 5 変種が
//! [`ProtocolServer::listener`] → [`ServerListener::cert`] → `spawn()` の 1 本に
//! 統合された。 現在の経路が cert を honor することを mesh 発行の Custom trust で
//! 検証する:
//!
//! - server: `listener(addr).cert(mesh_cert).spawn()` で mesh SelfSigned cert を載せる
//! - client: `TrustAnchors::Custom([mesh CA])` でその CA を信頼 (= SkipVerification 不使用)
//!
//! spawn 経路が cert を無視して dev_localhost に fallback していた旧実装では、
//! client が信頼するのは mesh CA なので cert mismatch で handshake が落ちる。
//! 本テストが通ること自体が「spawn 経路が cert を honor する」証拠。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行 (実 QUIC runtime 必須)。
//!
//! [`ServerListener::cert`]: unison::network::ServerListener::cert

use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::Level;

use unison::network::cert::CertSource;
use unison::network::channel::UnisonChannel;
use unison::network::{
    InternalMeshKeypair, MessageType, ProtocolClient, ProtocolServer, QuicClient,
};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// ping-pong サーバーを高レベル spawn 経路 (`listener(..).cert(..).spawn()`) で
/// 起動し、 `ServerHandle` と接続用アドレスを返す。 manual builder ではなくこの
/// 経路を通すのが本テストの主眼 (= gap が塞がった経路の検証)。
async fn spawn_with_cert(
    cert: CertSource,
    bind: &str,
) -> Result<(unison::network::ServerHandle, String)> {
    let server = ProtocolServer::with_identity("spawn-cert-e2e", "1.0.0", "test");
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

    let handle = Arc::new(server).listener(bind).cert(cert).spawn().await?;
    let local = handle.local_addr();
    let connect_str = format!("[{}]:{}", local.ip(), local.port());
    Ok((handle, connect_str))
}

// ─────────────────────────────────────────────────
// positive: listener(..).cert(..) に渡した mesh cert が適用され、
// Custom trust client が round-trip できる (= 旧 dev_localhost 固定なら落ちる)
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn spawn_path_honors_cert_source() -> Result<()> {
    init_tracing();

    // SAN = "::1" (= 接続先と一致する IP SAN)。 mesh が server cert + client CA を払い出す。
    let pair = InternalMeshKeypair::generate(["::1".to_string()])?;
    let (handle, addr) = spawn_with_cert(pair.server_cert_source, "[::1]:0").await?;

    let quic = QuicClient::builder()
        .trust_anchors(pair.client_trust_anchors)
        .build()?;
    let client = ProtocolClient::new(quic);

    // 旧実装 (spawn が dev_localhost 固定) なら、 client が信頼するのは mesh CA なので
    // cert mismatch で handshake が落ちる。 通れば spawn が cert を honor している証拠。
    client.connect(&addr).await?;
    assert!(
        client.is_connected().await,
        "listener(..).cert(..) に渡した cert で接続成功すべき"
    );

    let channel = client.open_channel("ping-pong").await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("ping", &json!({ "message": "spawn-cert" })),
    )
    .await??;
    assert_eq!(
        resp.get("message").and_then(|v| v.as_str()),
        Some("Pong: spawn-cert")
    );

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
