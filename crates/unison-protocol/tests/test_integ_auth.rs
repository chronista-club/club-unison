//! Large × E2E: connection-level auth primitive (`unison.auth`) round-trip
//!
//! Server に [`ProtocolServer::enable_auth`] で verifier (= policy) を注入して起動し、
//! client が credential を提示して認証 → 以降の channel が `ctx.principal()` で
//! authZ gate されることを実 QUIC stack 上で確認する。
//!
//! 設計: `design/connection-auth.md`
//! SSOT memory: mem_1CcTT4yxguA1KjGJXXHFor / handoff: mem_1CcTTLKuuTYGfATSKdSo8J
//!
//! すべて `#[ignore = "Large: E2E test"]` 付き — `cargo test -- --ignored` で実行。

use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use unison::network::channel::UnisonChannel;
use unison::network::{MessageType, Principal, ProtocolClient, ProtocolServer, ServerHandle};

const GOOD_TOKEN: &[u8] = b"good-token";
const SECRET_CHANNEL: &str = "secret";

/// app の principal 型 (= policy 側が定義、 library は知らない opaque 値)
#[derive(Debug, Clone)]
struct TestUser {
    name: String,
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();
}

/// gated channel handler: `ctx.principal()` を引いて authZ gate する (= per-message)。
/// 認証済みなら principal の名前を返し、 未認証なら "unauthenticated" を返す。
async fn handle_secret(
    ctx: Arc<unison::network::context::ConnectionContext>,
    stream: unison::network::UnisonStream,
) -> Result<(), unison::network::NetworkError> {
    let channel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                // app 側の downcast (= mechanism は opaque を渡すだけ)
                let user = ctx
                    .principal()
                    .await
                    .and_then(|p| p.downcast_ref::<TestUser>().map(|u| u.name.clone()));
                let payload = match user {
                    Some(name) => json!({ "authed": true, "whoami": name }),
                    None => json!({ "authed": false }),
                };
                channel.send_response(msg.id, &msg.method, &payload).await?;
            }
            Ok(_) => {}
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

/// verifier (= policy): GOOD_TOKEN だけ通し、 TestUser principal を返す。
async fn start_auth_server() -> Result<(ServerHandle, String)> {
    let server = ProtocolServer::with_identity("test-auth-srv", "0.1.0", "test");
    server
        .enable_auth(|credential: Vec<u8>| async move {
            (credential == GOOD_TOKEN).then(|| {
                Arc::new(TestUser {
                    name: "alice".into(),
                }) as Principal
            })
        })
        .await;
    server.register_channel(SECRET_CHANNEL, handle_secret).await;
    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();
    Ok((handle, format!("[{}]:{}", addr.ip(), addr.port())))
}

// ─────────────────────────────────────────────────────────────────────
// Test 1: 正当 credential → principal set → gated method 通過
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_auth_valid_credential_passes_gate() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_auth_server().await?;

    let client = ProtocolClient::new_default()?;
    client.connect_with_credential(&addr, GOOD_TOKEN).await?;

    let channel = client.open_channel(SECRET_CHANNEL).await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("GetSecret", &json!({})),
    )
    .await??;

    assert_eq!(resp["authed"], json!(true), "authed should be true");
    assert_eq!(
        resp["whoami"],
        json!("alice"),
        "principal name should leak through downcast"
    );

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 2: 不正 credential → connect_with_credential が拒否される
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_auth_invalid_credential_rejected() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_auth_server().await?;

    let client = ProtocolClient::new_default()?;
    let result = client.connect_with_credential(&addr, b"wrong-token").await;

    assert!(
        result.is_err(),
        "invalid credential should be rejected by verifier"
    );

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 3: 未認証 (plain connect) → principal None → gated method が拒否反応
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_auth_unauthenticated_is_gated() -> Result<()> {
    init_tracing();
    let (handle, addr) = start_auth_server().await?;

    // credential を出さず素の connect → principal は立たない
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;

    let channel = client.open_channel(SECRET_CHANNEL).await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("GetSecret", &json!({})),
    )
    .await??;

    assert_eq!(
        resp["authed"],
        json!(false),
        "unauthenticated connection should be gated (principal None)"
    );

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Test 4: enable_auth を呼ばない server は従来通り動く (= opt-in、 非破壊)
// ─────────────────────────────────────────────────────────────────────
#[tokio::test]
#[ignore = "Large: E2E test"]
async fn test_e2e_no_enable_auth_is_nonbreaking() -> Result<()> {
    init_tracing();

    // enable_auth 無し。通常 channel のみ。
    let server = ProtocolServer::with_identity("test-noauth-srv", "0.1.0", "test");
    server
        .register_channel("echo", |_ctx, stream| async move {
            let channel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request => {
                        let value = msg.payload_as_value().unwrap_or_default();
                        channel.send_response(msg.id, &msg.method, &value).await?;
                    }
                    Ok(_) => {}
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
        })
        .await;
    let handle = server.spawn_listen("[::1]:0").await?;
    let addr = handle.local_addr();
    let addr = format!("[{}]:{}", addr.ip(), addr.port());

    // 素の connect で従来通り動作する
    let client = ProtocolClient::new_default()?;
    client.connect(&addr).await?;
    let channel = client.open_channel("echo").await?;
    let resp: Value = timeout(
        Duration::from_secs(5),
        channel.request("Echo", &json!({ "msg": "hi" })),
    )
    .await??;
    assert_eq!(resp["msg"], json!("hi"));

    channel.close().await?;
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
