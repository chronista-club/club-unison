//! Medium x Integration: datagram channel echo テスト (v0.10.0)
//!
//! `ProtocolServer::register_channel_datagram` + `ProtocolClient::open_datagram_channel`
//! が end-to-end で繋がっているかを QUIC ペアで検証する。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行。 datagram は unreliable なので
//! 一定の drop tolerance を持たせて複数 attempt する。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{Level, info};

use unison::{ProtocolClient, ProtocolServer};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init();
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Transform {
    id: String,
    x: f32,
    y: f32,
    z: f32,
}

/// Echo handler を datagram channel `"position"` (channel_id=1) に登録
async fn register_echo_handler(server: &ProtocolServer) {
    server
        .register_channel_datagram("position", 1, |chan| async move {
            // 受信した Transform をそのまま echo back
            loop {
                match chan.recv_event::<Transform>().await {
                    Ok(transform) => {
                        // 同じ channel に send_event = 同じ connection に send_datagram
                        let _ = chan.send_event(&transform).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .await;
}

/// Server に handler を register → client が open + send_event → echo を受信できる
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_datagram_echo_round_trip() -> Result<()> {
    init_tracing();

    // ─── Server setup ──────────────────────────────────
    let server = Arc::new(ProtocolServer::new());
    register_echo_handler(&server).await;
    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let addr = handle.local_addr();
    info!("Server bound to {}", addr);

    // ─── Client connect ────────────────────────────────
    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;
    info!("Client connected");

    // Server 側の handle_connection が dispatcher を spawn してハンドラーを設置する
    // までの非同期 setup time を吸収 (= datagram は unreliable、 最初の 1-2 個は drop されても OK)
    tokio::time::sleep(Duration::from_millis(150)).await;

    // ─── Open datagram channel + echo ──────────────────
    let chan = client.open_datagram_channel("position", 1).await?;

    let transform = Transform {
        id: "player-1".to_string(),
        x: 1.0,
        y: 2.0,
        z: 3.0,
    };

    // 不確実性に備えて複数 attempt、 1 件でも echo が返ってきたら success
    let mut received: Option<Transform> = None;
    for attempt in 0..10 {
        chan.send_event(&transform).await?;
        // 100ms 待って echo を読む、 来なければ次 attempt
        match timeout(Duration::from_millis(100), chan.recv_event::<Transform>()).await {
            Ok(Ok(echo)) => {
                received = Some(echo);
                info!("Got echo on attempt {}", attempt + 1);
                break;
            }
            Ok(Err(_)) | Err(_) => {
                // timeout or recv error、 continue
            }
        }
    }

    let echo = received
        .expect("echo received within 10 attempts (= dropped 10 in a row is implausibly bad)");
    assert_eq!(echo, transform);

    // ─── Cleanup ───────────────────────────────────────
    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// 複数 datagram channel が同 connection で並列に動く (= channel_id demux 検証)
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_datagram_multiple_channels() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    // 2 つの datagram channel を登録
    server
        .register_channel_datagram("position", 1, |chan| async move {
            loop {
                match chan.recv_event::<Transform>().await {
                    Ok(t) => {
                        let _ = chan.send_event(&t).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .await;
    server
        .register_channel_datagram("presence", 2, |chan| async move {
            loop {
                match chan.recv_event::<Transform>().await {
                    Ok(t) => {
                        let _ = chan.send_event(&t).await;
                    }
                    Err(_) => break,
                }
            }
        })
        .await;

    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let addr = handle.local_addr();

    let client = ProtocolClient::new_default()?;
    client
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;
    tokio::time::sleep(Duration::from_millis(150)).await;

    let pos_chan = client.open_datagram_channel("position", 1).await?;
    let pres_chan = client.open_datagram_channel("presence", 2).await?;
    assert_eq!(pos_chan.channel_id(), 1);
    assert_eq!(pres_chan.channel_id(), 2);

    let t1 = Transform {
        id: "a".to_string(),
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };
    let t2 = Transform {
        id: "b".to_string(),
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };

    // 双方の channel で echo (= demux 正しく分離されること)
    let mut got_pos: Option<Transform> = None;
    let mut got_pres: Option<Transform> = None;
    for _ in 0..15 {
        if got_pos.is_none() {
            pos_chan.send_event(&t1).await?;
            if let Ok(Ok(e)) = timeout(
                Duration::from_millis(80),
                pos_chan.recv_event::<Transform>(),
            )
            .await
            {
                got_pos = Some(e);
            }
        }
        if got_pres.is_none() {
            pres_chan.send_event(&t2).await?;
            if let Ok(Ok(e)) = timeout(
                Duration::from_millis(80),
                pres_chan.recv_event::<Transform>(),
            )
            .await
            {
                got_pres = Some(e);
            }
        }
        if got_pos.is_some() && got_pres.is_some() {
            break;
        }
    }

    assert_eq!(got_pos.expect("position echo"), t1);
    assert_eq!(got_pres.expect("presence echo"), t2);

    client.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}

/// Server.broadcast: 全 active connection に同 event を送る
#[tokio::test]
#[ignore = "Medium: requires QUIC runtime"]
async fn test_medium_datagram_broadcast_to_all_clients() -> Result<()> {
    init_tracing();

    let server = Arc::new(ProtocolServer::new());
    // handler は不要 (= broadcast の宛先は client 側の DatagramChannel)、
    // ただし name → channel_id の registry が必要なので noop handler を register
    server
        .register_channel_datagram("position", 1, |_chan| async {
            // noop (= broadcast 専用、 server は受信しない)
        })
        .await;

    let handle = Arc::clone(&server).spawn_listen_shared("[::1]:0").await?;
    let addr = handle.local_addr();

    // 2 client を connect
    let client_a = ProtocolClient::new_default()?;
    let client_b = ProtocolClient::new_default()?;
    client_a
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;
    client_b
        .connect(&format!("[{}]:{}", addr.ip(), addr.port()))
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.active_connection_count().await, 2);

    let chan_a = client_a.open_datagram_channel("position", 1).await?;
    let chan_b = client_b.open_datagram_channel("position", 1).await?;

    let t = Transform {
        id: "broadcast".to_string(),
        x: 7.0,
        y: 8.0,
        z: 9.0,
    };

    let mut got_a: Option<Transform> = None;
    let mut got_b: Option<Transform> = None;
    for _ in 0..15 {
        let n = server
            .broadcast::<_, unison::codec::JsonCodec>("position", &t)
            .await?;
        assert!(n <= 2, "broadcast can hit at most 2 connections");
        if got_a.is_none()
            && let Ok(Ok(e)) =
                timeout(Duration::from_millis(80), chan_a.recv_event::<Transform>()).await
        {
            got_a = Some(e);
        }
        if got_b.is_none()
            && let Ok(Ok(e)) =
                timeout(Duration::from_millis(80), chan_b.recv_event::<Transform>()).await
        {
            got_b = Some(e);
        }
        if got_a.is_some() && got_b.is_some() {
            break;
        }
    }

    assert_eq!(got_a.expect("client A receives broadcast"), t);
    assert_eq!(got_b.expect("client B receives broadcast"), t);

    client_a.disconnect().await?;
    client_b.disconnect().await?;
    handle.shutdown().await?;
    Ok(())
}
