//! 実 WebTransport echo サーバー (= Phase 6d Step 1)。
//!
//! TS SDK ⇄ Rust server の **実 WebTransport ラウンドトリップ** を検証するための
//! dev サーバー。 自己署名証明書で WebTransport ingress を開き、 2 つの channel を
//! 登録する:
//!
//! - `echo` — request/response。 `Echo` request の `text` をそのまま返す。
//! - `clock` — event push。 channel open 後、 200ms ごとに `Tick` event を 3 回送る。
//!
//! 起動時に **leaf 証明書の SHA-256 hex hash** を stdout に決まった形式で印字する
//! (`CERT_HASH=<64 hex>`)。 client (TS SDK / ブラウザ) はこの値で自己署名証明書を
//! `serverCertificateHashes` pin する。
//!
//! ```text
//! cargo run -p club-unison --example webtransport_echo_server -- [addr]
//! ```
//!
//! `addr` 省略時は `[::1]:4433`。 stdout は client が parse する契約なので、 log は
//! stderr へ流す (= `RUST_LOG` で制御)。

use std::sync::Arc;

use anyhow::{Context, Result};
use unison::ProtocolServer;
use unison::network::quic::UnisonStream;
use unison::network::webtransport::WebTransportServer;
use unison::network::{MessageType, UnisonChannel};

#[tokio::main]
async fn main() -> Result<()> {
    // log は stderr へ。 stdout は CERT_HASH / READY 行の契約に温存する。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "[::1]:4433".to_string());

    // ── channel 登録 ─────────────────────────────────────────
    let server = ProtocolServer::with_identity(
        "unison-webtransport-echo",
        env!("CARGO_PKG_VERSION"),
        "example",
    );

    // echo channel: request/response。 `Echo` request の `text` をそのまま返す。
    server
        .register_channel(
            "echo",
            |_ctx, stream| async move { handle_echo(stream).await },
        )
        .await;

    // clock channel: event push。 open 後に `Tick` event を 200ms 間隔で 3 回送る。
    server
        .register_channel(
            "clock",
            |_ctx, stream| async move { handle_clock(stream).await },
        )
        .await;

    // ── WebTransport ingress 起動 ────────────────────────────
    let mut wt = WebTransportServer::dev(Arc::new(server));
    let socket_addr = addr
        .parse()
        .with_context(|| format!("bind addr の parse に失敗: {addr}"))?;
    wt.bind(socket_addr)
        .await
        .context("WebTransport ingress の bind に失敗")?;

    let cert_hash = wt
        .certificate_hash_hex()
        .context("bind 後は cert hash が確定しているべき")?;
    let local = wt
        .local_addr()
        .context("bind 後は local_addr が確定しているべき")?;

    // stdout 契約: client (E2E test) がこの 2 行を parse する。
    println!("CERT_HASH={cert_hash}");
    println!("READY addr=https://{local}");
    use std::io::Write;
    std::io::stdout().flush().ok();

    // 終了までブロック。
    wt.start()
        .await
        .context("WebTransport server の実行に失敗")?;
    Ok(())
}

/// `echo` channel: request の `text` をそのまま response で返す loop。
async fn handle_echo(stream: UnisonStream) -> Result<(), unison::network::NetworkError> {
    let channel: UnisonChannel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                // payload を JSON として decode し `text` を取り出してエコー。
                let req: serde_json::Value =
                    serde_json::from_slice(&msg.payload).unwrap_or(serde_json::Value::Null);
                let text = req.get("text").cloned().unwrap_or(serde_json::Value::Null);
                let reply = serde_json::json!({ "text": text });
                tracing::info!(method = %msg.method, "echo reply");
                channel.send_response(msg.id, &msg.method, &reply).await?;
            }
            Ok(msg) => {
                tracing::debug!(method = %msg.method, "echo ignored non-request");
            }
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

/// `clock` channel: open 後に `Tick` event を 200ms 間隔で 3 回 push する。
async fn handle_clock(stream: UnisonStream) -> Result<(), unison::network::NetworkError> {
    let channel: UnisonChannel = UnisonChannel::new(stream);
    for seq in 0..3u32 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let event = serde_json::json!({ "seq": seq });
        tracing::info!(seq, "clock tick");
        channel.send_event("Tick", &event).await?;
    }
    Ok(())
}
