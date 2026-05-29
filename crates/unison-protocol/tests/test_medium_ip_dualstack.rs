//! Medium x Integration: IPv4 / IPv6 デュアルスタック疎通テスト
//!
//! Unison は **原則 IPv6 運用**。ただし IPv4 / IPv6 の両対応が前提なので、
//! 両 family の loopback 疎通と「混在」(1 プロセスが v6/v4 両サーバーに接続)
//! を決定的に検証する。
//!
//! 「混在」は OS 依存になりがちな dual-stack unspecified bind
//! (`[::]` で IPv4-mapped を受理するか) に依存させず、
//! **v6 サーバー + v4 サーバーを同時起動し、同一プロセスのクライアントが
//! 両方と round-trip する** 形で検証する。これにより per-target-family の
//! client bind ロジック (V4→`0.0.0.0:0` / V6→`[::]:0`、quic.rs) を
//! プラットフォーム非依存で担保する。
//!
//! すべて `#[ignore]` 付き — `cargo test -- --ignored` で実行。
//!
//! テストリスト (IPv6-first):
//! - [x] ipv6_loopback_roundtrip          : 既定経路 (`[::1]`)
//! - [x] ipv4_loopback_roundtrip          : 互換経路 (`127.0.0.1`)
//! - [x] mixed_client_reaches_v6_and_v4   : 混在 (1 client → v6 server + v4 server)
//! - [x] ipv6_unspecified_bind_is_v6      : 運用既定 (`[::]:0` は v6 として listen)
//! - [x] ipv4_unspecified_bind_is_v4      : 互換 (`0.0.0.0:0` は v4 として listen)

use anyhow::Result;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::network::MessageType;
use unison::network::channel::UnisonChannel;
use unison::{ProtocolClient, ProtocolServer, ServerHandle};

/// テスト用のトレーシング初期化（複数テストで呼ばれても安全）
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// 指定 bind アドレスで ping-pong エコーサーバーを起動し、
/// `ServerHandle` と family 正規化済みの接続用アドレス文字列を返す。
async fn start_echo_server_on(bind: &str) -> Result<(ServerHandle, ServerAddr)> {
    let server = ProtocolServer::with_identity("dualstack-test", "1.0.0", "test");

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

    let handle = server.spawn_listen(bind).await?;
    let local = handle.local_addr();
    info!(
        "dualstack echo server listening on {} (bind={})",
        local, bind
    );
    Ok((handle, ServerAddr(local)))
}

/// サーバーの実アドレスから「クライアントが接続できる loopback アドレス文字列」を作る。
///
/// `[::]:0` / `0.0.0.0:0` のような unspecified bind ではそのアドレスに
/// 接続できないため、同 family の loopback (`[::1]` / `127.0.0.1`) に
/// 割り当て済みポートを組み合わせて返す。
struct ServerAddr(std::net::SocketAddr);

impl ServerAddr {
    fn connect_str(&self) -> String {
        let port = self.0.port();
        match self.0.ip() {
            std::net::IpAddr::V6(ip) => {
                let host = if ip.is_unspecified() {
                    std::net::Ipv6Addr::LOCALHOST
                } else {
                    ip
                };
                format!("[{host}]:{port}")
            }
            std::net::IpAddr::V4(ip) => {
                let host = if ip.is_unspecified() {
                    std::net::Ipv4Addr::LOCALHOST
                } else {
                    ip
                };
                format!("{host}:{port}")
            }
        }
    }
}

/// 接続済みクライアントで ping-pong チャネルを開き、1 往復して応答を検証する。
async fn assert_ping_pong(client: &ProtocolClient) -> Result<()> {
    let channel = client.open_channel("ping-pong").await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("ping", &json!({ "message": "dualstack" })),
    )
    .await??;
    assert_eq!(
        resp.get("message").and_then(|v| v.as_str()),
        Some("Pong: dualstack"),
        "ping-pong round-trip should echo the message"
    );
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 1: IPv6 loopback (既定経路)
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn ipv6_loopback_roundtrip() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_echo_server_on("[::1]:0").await?;
    assert!(handle.local_addr().is_ipv6(), "[::1] should bind as IPv6");

    let client = ProtocolClient::new_default()?;
    client.connect(&addr.connect_str()).await?;
    assert!(client.is_connected().await);

    assert_ping_pong(&client).await?;

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 2: IPv4 loopback (互換経路)
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn ipv4_loopback_roundtrip() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_echo_server_on("127.0.0.1:0").await?;
    assert!(
        handle.local_addr().is_ipv4(),
        "127.0.0.1 should bind as IPv4"
    );

    let client = ProtocolClient::new_default()?;
    client.connect(&addr.connect_str()).await?;
    assert!(client.is_connected().await);

    assert_ping_pong(&client).await?;

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 3: 混在 — 1 プロセスが v6 / v4 両サーバーと round-trip
// ─────────────────────────────────────────────────

/// v6 サーバーと v4 サーバーを同時起動し、同一プロセス内の 2 クライアントが
/// それぞれ別 family のサーバーへ接続して round-trip する。
/// V4→`0.0.0.0:0` / V6→`[::]:0` の per-family client bind が同居で破綻しないことを保証。
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn mixed_client_reaches_v6_and_v4() -> Result<()> {
    init_tracing();

    let (v6_handle, v6_addr) = start_echo_server_on("[::1]:0").await?;
    let (v4_handle, v4_addr) = start_echo_server_on("127.0.0.1:0").await?;
    assert!(v6_handle.local_addr().is_ipv6());
    assert!(v4_handle.local_addr().is_ipv4());

    // IPv6 サーバーへ
    let v6_client = ProtocolClient::new_default()?;
    v6_client.connect(&v6_addr.connect_str()).await?;
    assert!(v6_client.is_connected().await, "v6 client should connect");
    assert_ping_pong(&v6_client).await?;

    // IPv4 サーバーへ (同一プロセス、別 family)
    let v4_client = ProtocolClient::new_default()?;
    v4_client.connect(&v4_addr.connect_str()).await?;
    assert!(v4_client.is_connected().await, "v4 client should connect");
    assert_ping_pong(&v4_client).await?;

    v6_client.disconnect().await?;
    v4_client.disconnect().await?;
    v6_handle.shutdown().await?;
    v4_handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 4: 運用既定 — unspecified IPv6 bind (`[::]:0`) は v6 として listen
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn ipv6_unspecified_bind_is_v6() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_echo_server_on("[::]:0").await?;
    assert!(
        handle.local_addr().is_ipv6(),
        "[::] unspecified bind should listen as IPv6"
    );

    let client = ProtocolClient::new_default()?;
    client.connect(&addr.connect_str()).await?; // -> [::1]:port
    assert!(client.is_connected().await);
    assert_ping_pong(&client).await?;

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────
// Test 5: 互換 — unspecified IPv4 bind (`0.0.0.0:0`) は v4 として listen
// ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn ipv4_unspecified_bind_is_v4() -> Result<()> {
    init_tracing();

    let (handle, addr) = start_echo_server_on("0.0.0.0:0").await?;
    assert!(
        handle.local_addr().is_ipv4(),
        "0.0.0.0 unspecified bind should listen as IPv4"
    );

    let client = ProtocolClient::new_default()?;
    client.connect(&addr.connect_str()).await?; // -> 127.0.0.1:port
    assert!(client.is_connected().await);
    assert_ping_pong(&client).await?;

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
