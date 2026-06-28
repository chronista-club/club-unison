//! Medium x Integration: server-initiated reliable stream（1.5.0）
//!
//! `ConnectionContext::open_server_stream`（server 起点で stream を開く）+
//! `ProtocolClient::register_server_channel`（client が raw `UnisonStream` handler を登録）が
//! end-to-end で繋がり、**取りこぼし無し・同順**で配送されることを QUIC ペアで検証する。
//!
//! 設計: `design/server-initiated-stream.md`（対称化路線 = client handler も server と同じ
//! raw `UnisonStream` を直読する → recv ループ／中継 mpsc を挟まないので drop も OOM も起きない）。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行（QUIC runtime が要る）。

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::Level;

use unison::network::context::ConnectionContext;
use unison::network::{ConnectionEvent, MessageType, ProtocolMessage};
use unison::{ProtocolClient, ProtocolServer};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

/// Connected event を待って、その connection の ctx を取り出す。
async fn wait_for_ctx(
    events: &mut unison::network::ConnectionEventReceiver,
) -> Arc<ConnectionContext> {
    loop {
        match events.recv().await.expect("connection event") {
            ConnectionEvent::Connected { context, .. } => return context,
            ConnectionEvent::Disconnected { .. } => continue,
        }
    }
}

/// server が `open_server_stream` で push した N 件を、client handler が
/// **全件・同順**で受信する（reliable server→client）。
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_server_initiated_reliable_ordered_delivery() -> Result<()> {
    init_tracing();
    const N: i64 = 200;

    // ─── Server setup ──────────────────────────────────
    let server = Arc::new(ProtocolServer::new());
    let mut events = server.subscribe_connection_events();
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let addr = handle.local_addr();

    // ─── Client: handler を connect 前に登録 ────────────
    let (tx, mut rx) = mpsc::unbounded_channel::<i64>();
    let client = ProtocolClient::new_default()?;
    client
        .register_server_channel("relay", move |stream| {
            let tx = tx.clone();
            async move {
                // raw UnisonStream を直読（= QUIC backpressure、取りこぼし無し）
                loop {
                    match stream.recv_frame().await {
                        Ok(msg) => {
                            let v = msg.payload_as_value()?;
                            let seq = v.get("seq").and_then(|s| s.as_i64()).unwrap_or(-1);
                            let _ = tx.send(seq);
                        }
                        Err(_) => break, // end of stream（server が close）
                    }
                }
                Ok(())
            }
        })
        .await;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;

    // ─── Server: ctx を得て reliable push stream に N 件送る ──
    let ctx = wait_for_ctx(&mut events).await;
    let stream = ctx.open_server_stream("relay").await?;
    for i in 0..N {
        let msg = ProtocolMessage::new_with_json(
            0,
            "msg".to_string(),
            MessageType::Event,
            serde_json::json!({ "seq": i }),
        )?;
        stream.send_frame(&msg).await?;
    }

    // ─── Client: 全件を順序どおり受信 ──────────────────
    let mut got = Vec::new();
    for _ in 0..N {
        match timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(seq)) => got.push(seq),
            Ok(None) => break, // sender 終了
            Err(_) => break,   // timeout
        }
    }

    assert_eq!(
        got.len() as i64,
        N,
        "全 {N} 件が reliable に届くべき（取りこぼし無し）。received={}",
        got.len()
    );
    let expected: Vec<i64> = (0..N).collect();
    assert_eq!(got, expected, "同順で届くべき（単一 QUIC stream の順序保証）");

    // ─── Cleanup ───────────────────────────────────────
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// handler 未登録の channel へ push しても、client は drop + warn するだけで
/// 接続は壊れない（後方互換 = 無回帰）。
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_server_initiated_unregistered_channel_no_regression() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    let mut events = server.subscribe_connection_events();
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    // handler は登録しない
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;

    let ctx = wait_for_ctx(&mut events).await;

    // 未登録 channel へ push → client 側は drop + warn（panic しない）
    let stream = ctx.open_server_stream("no-handler").await?;
    let msg = ProtocolMessage::new_with_json(
        0,
        "x".to_string(),
        MessageType::Event,
        serde_json::json!({}),
    )?;
    stream.send_frame(&msg).await?;

    // 接続は維持されている: connect 時に受けた identity が依然取得できる
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        client.server_identity().await.is_some(),
        "未登録 server stream を drop しても接続は壊れない"
    );

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// client 側 ctx（conn 未 set）で `open_server_stream` を呼ぶと誤用として error。
/// QUIC runtime 不要。
#[tokio::test]
async fn test_open_server_stream_on_client_ctx_is_misuse_error() {
    let ctx = ConnectionContext::new();
    let res = ctx.open_server_stream("x").await;
    assert!(
        res.is_err(),
        "conn 未 set の ctx（client 側）では open_server_stream は誤用 error になるべき"
    );
}
