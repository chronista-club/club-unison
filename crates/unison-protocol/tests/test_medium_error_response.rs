//! Medium x Integration: handler が request を断る (`UnisonChannel::send_error`)
//!
//! handler が request を処理できないとき、 成功の response に失敗を詰めるのではなく、
//! error frame で返せること、 client の `request()` がその理由を error として受け取ることを
//! QUIC ペアで検証する。
//!
//! `#[ignore]` 付き — `cargo test -- --ignored` で実行（QUIC runtime が要る）。

use anyhow::Result;
use serde_json::json;
use std::sync::Arc;

use unison::network::{MessageType, NetworkError, UnisonChannel};
use unison::{ProtocolClient, ProtocolServer};

#[tokio::test]
#[ignore = "Medium: 実 QUIC runtime が要る"]
async fn handler_can_refuse_a_request_with_a_reason() -> Result<()> {
    let server = Arc::new(ProtocolServer::new());
    server
        .register_channel("sessions", |_ctx, stream| async move {
            let channel: UnisonChannel = UnisonChannel::new(stream);
            loop {
                match channel.recv().await {
                    Ok(msg) if msg.msg_type == MessageType::Request => {
                        if msg.method == "Remove" {
                            channel
                                .send_error(
                                    msg.id,
                                    &msg.method,
                                    &json!({ "error": "root session は取り除けません" }),
                                )
                                .await?;
                        } else {
                            channel
                                .send_response(msg.id, &msg.method, &json!({ "ok": true }))
                                .await?;
                        }
                    }
                    Ok(_) => {}
                    Err(e) if e.is_normal_close() => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
        })
        .await;
    let handle = Arc::clone(&server).listener("[::1]:0").spawn().await?;

    let client = ProtocolClient::insecure_localhost()?;
    client.connect(&handle.local_addr().to_string()).await?;
    let channel = client.open_channel("sessions").await?;

    let refused: Result<serde_json::Value, NetworkError> = channel
        .request("Remove", &json!({ "session": "root" }))
        .await;
    match refused {
        Err(NetworkError::Protocol(message)) => {
            assert!(
                message.contains("root session は取り除けません"),
                "断られた理由が error に入っているべき: {message}"
            );
        }
        other => panic!("error frame は Protocol error として届くべき: {other:?}"),
    }

    // 断ったあとも、 同じ channel で次の request は通る。
    let ok: serde_json::Value = channel.request("List", &json!({})).await?;
    assert_eq!(ok, json!({ "ok": true }));

    channel.close().await?;
    handle.shutdown().await?;
    Ok(())
}
