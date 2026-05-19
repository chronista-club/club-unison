//! `unison ping <url>` — サーバへ接続して疎通 + RTT 計測。
//!
//! QUIC connect = Identity Handshake まで含むため、connect() の所要時間が
//! 概ね handshake round-trip に相当する。複数回計測して min/avg/max を出す。

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use unison::ProtocolClient;
use unison::network::quic::QuicClient;

use crate::TrustMode;

#[derive(Args)]
pub struct PingArgs {
    /// Unison サーバの URL (例: `quic://[::1]:7878`)
    pub url: String,

    /// trust anchor mode (default: skip — dev self-signed 用)
    #[arg(long, value_enum, default_value = "skip")]
    pub trust: TrustMode,

    /// 計測回数
    #[arg(short, long, default_value_t = 4)]
    pub count: u32,
}

pub async fn run(args: PingArgs) -> Result<()> {
    println!("PING {} (trust={:?})", args.url, args.trust);

    let mut samples: Vec<Duration> = Vec::with_capacity(args.count as usize);
    let mut identity_shown = false;
    for seq in 1..=args.count {
        match probe_once(&args).await {
            Ok((rtt, identity)) => {
                if !identity_shown {
                    match &identity {
                        Some(id) => println!("  server: {id}"),
                        None => println!("  server: <identity 未受信>"),
                    }
                    identity_shown = true;
                }
                println!("  seq={seq:<3} connected  time={:.2}ms", ms(rtt));
                samples.push(rtt);
            }
            Err(e) => {
                println!("  seq={seq:<3} unreachable — {e:#}");
            }
        }
    }

    println!();
    let lost = args.count as usize - samples.len();
    let loss_pct = lost as f64 / args.count as f64 * 100.0;
    println!(
        "--- {} statistics ---\n{} probes, {} ok, {} failed, {:.0}% loss",
        args.url,
        args.count,
        samples.len(),
        lost,
        loss_pct,
    );

    if let (Some(min), Some(max)) = (samples.iter().min(), samples.iter().max()) {
        let avg = samples.iter().sum::<Duration>() / samples.len() as u32;
        println!(
            "rtt min/avg/max = {:.2}/{:.2}/{:.2} ms",
            ms(*min),
            ms(avg),
            ms(*max),
        );
    }

    if samples.is_empty() {
        anyhow::bail!("server {} unreachable", args.url);
    }
    Ok(())
}

/// 1 回 connect して RTT と server identity を返す。
async fn probe_once(args: &PingArgs) -> Result<(Duration, Option<String>)> {
    let quic = QuicClient::builder()
        .trust_anchors(args.trust.to_anchors())
        .build()
        .context("QUIC client init failed")?;
    let client = ProtocolClient::new(quic);

    let start = Instant::now();
    client.connect(&args.url).await.context("connect failed")?;
    let rtt = start.elapsed();

    // Identity Handshake で受信済みの server identity (= 接続先の確認用)
    let identity = client
        .server_identity()
        .await
        .map(|id| format!("{} v{} ({})", id.name, id.version, id.namespace));

    let _ = client.disconnect().await;
    Ok((rtt, identity))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
