//! Quinn-backed fixture for the Swift manual handover probe.
//!
//! cargo run -p club-unison --example handover_probe_server -- '[::1]:17878'
//! Prints a public leaf certificate pin; private key material stays in memory.
//! See guides/swift-handover.md before binding a remotely reachable address.

use std::io::Write;
use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use unison::ProtocolServer;
use unison::network::cert::CertSource;
use unison::network::{MessageType, NetworkError, UnisonChannel};
use uuid::Uuid;

#[derive(Deserialize)]
struct Probe {
    seq: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "[::1]:17878".into());
    let server = ProtocolServer::with_identity("handover-probe", "1", "diagnostics");
    server
        .register_channel("handover-probe", |ctx, stream| async move {
            let channel: UnisonChannel = UnisonChannel::new(stream);
            // A new channel gets a new ID, even on the same QUIC connection.
            let channel_id = Uuid::new_v4();
            let mut received = 0_u64;
            loop {
                let msg = match channel.recv().await {
                    Ok(msg) => msg,
                    Err(err) if err.is_normal_close() => return Ok(()),
                    Err(err) => return Err(err),
                };
                if msg.msg_type != MessageType::Request || msg.method != "Probe" {
                    return Err(NetworkError::Protocol("expected Probe request".into()));
                }
                let probe: Probe = serde_json::from_slice(&msg.payload)?;
                received += 1;
                let reply = serde_json::json!({
                    "seq": probe.seq,
                    "received": received,
                    "connection_id": ctx.connection_id,
                    "channel_id": channel_id,
                });
                tracing::info!(
                    connection_id = %ctx.connection_id,
                    %channel_id,
                    seq = probe.seq,
                    received,
                    "probe"
                );
                channel.send_response(msg.id, &msg.method, &reply).await?;
            }
        })
        .await;

    let key = CertSource::internal_mesh(["handover-probe".into()]).resolve()?;
    let pin: String = key.cert[0]
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let handle = Arc::new(server)
        .listener(&addr)
        .cert(CertSource::Provided { certified_key: key })
        .spawn()
        .await?;
    println!("CERT_HEX={pin}");
    println!("READY addr={}", handle.local_addr());
    std::io::stdout().flush()?;
    tokio::signal::ctrl_c().await?;
    handle.shutdown().await?;
    Ok(())
}
