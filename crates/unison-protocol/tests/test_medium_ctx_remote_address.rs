//! Medium x Integration: `ConnectionContext::remote_address`
//!
//! channel handler が、 受け取った ctx からその接続の相手のアドレスを引けることを
//! QUIC ペアで検証する (= 「loopback からの接続にだけ答える」 のような判定を、
//! listener を分けずに handler の中で書けるようにするため)。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行（QUIC runtime が要る）。

use anyhow::Result;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use unison::network::{ConnectionEvent, MessageType, UnisonChannel};
use unison::{ProtocolClient, ProtocolServer};

/// handler が見た相手のアドレスは、 接続イベントの `remote_addr` と同じ loopback の
/// アドレスになる。 client 側の ctx は相手のアドレスを持たない。
#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn handler_sees_the_peer_address_of_its_connection() -> Result<()> {
    let server = Arc::new(ProtocolServer::new());
    server
        .register_channel("whoami", |ctx, stream| async move {
            let channel: UnisonChannel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request => {
                        let peer = ctx.remote_address().await.map(|a| a.to_string());
                        channel
                            .send_response(msg.id, &msg.method, &json!({ "peer": peer }))
                            .await?;
                    }
                    Ok(_) => {}
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
        })
        .await;
    let mut events = server.subscribe_connection_events();
    let handle = Arc::clone(&server).listener("[::1]:0").spawn().await?;

    let client = ProtocolClient::insecure_localhost()?;
    client.connect(&handle.local_addr().to_string()).await?;
    let connected_from = loop {
        match timeout(Duration::from_secs(5), events.recv()).await?? {
            ConnectionEvent::Connected { remote_addr, .. } => break remote_addr,
            ConnectionEvent::Disconnected { .. } => continue,
        }
    };

    let channel = client.open_channel("whoami").await?;
    let reply: serde_json::Value = channel.request("WhoAmI", &json!({})).await?;
    let seen: SocketAddr = reply["peer"]
        .as_str()
        .expect("server 側の ctx は相手のアドレスを持つ")
        .parse()?;

    assert!(seen.ip().is_loopback(), "loopback からの接続のはず: {seen}");
    assert_eq!(seen, connected_from);
    assert_eq!(
        client.context().remote_address().await,
        None,
        "client 側の ctx は conn を持たない"
    );

    channel.close().await?;
    handle.shutdown().await?;
    Ok(())
}
