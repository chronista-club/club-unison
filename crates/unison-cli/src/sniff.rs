//! `unison sniff <url>` — channel traffic を覗く dev packet inspector。
//!
//! 指定 channel を open し、サーバから push されてくる Event / 非 Response
//! メッセージを `UnisonChannel::recv()` で受け取り、到着順に整形表示する。
//!
//! 注: client 視点の inspector のため、観測できるのは「自分が開いた channel に
//! 流れてくるメッセージ」。connection 全体の全 stream を覗く wire-level tap は
//! unison-protocol 側に API が無く範囲外 (報告参照)。

use std::time::Instant;

use anyhow::{Context, Result};
use clap::Args;
use unison::ProtocolClient;
use unison::network::quic::QuicClient;

use crate::TrustMode;

#[derive(Args)]
pub struct SniffArgs {
    /// Unison サーバの URL (例: `quic://[::1]:7878` / `quic://127.0.0.1:7878`)
    pub url: String,

    /// 覗く channel 名
    #[arg(short, long)]
    pub channel: String,

    /// trust anchor mode (default: skip)
    #[arg(long, value_enum, default_value = "skip")]
    pub trust: TrustMode,

    /// この件数を観測したら終了 (0 = 無制限、Ctrl-C で停止)
    #[arg(short = 'n', long, default_value_t = 0)]
    pub limit: u64,
}

pub async fn run(args: SniffArgs) -> Result<()> {
    let quic = QuicClient::builder()
        .trust_anchors(args.trust.to_anchors())
        .build()
        .context("QUIC client init failed")?;
    let client = ProtocolClient::new(quic);

    client.connect(&args.url).await.context("connect failed")?;
    eprintln!(
        "connected to {} — opening channel '{}'",
        args.url, args.channel
    );

    let channel = client
        .open_channel(&args.channel)
        .await
        .context("open_channel failed")?;

    println!(
        "sniffing channel '{}' on {} — Ctrl-C to stop",
        args.channel, args.url
    );
    println!(
        "{:<10} {:>8} {:<9} {:<24} payload",
        "t(ms)", "id", "type", "method"
    );
    println!("{}", "-".repeat(72));

    let start = Instant::now();
    let mut seen: u64 = 0;
    loop {
        if args.limit != 0 && seen >= args.limit {
            break;
        }
        match channel.recv().await {
            Ok(msg) => {
                seen += 1;
                let payload = msg
                    .payload_as_value()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|_| format!("<{} raw bytes>", msg.payload.len()));
                println!(
                    "{:<10.1} {:>8} {:<9} {:<24} {}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    msg.id,
                    format!("{:?}", msg.msg_type),
                    truncate(&msg.method, 24),
                    truncate(&payload, 120),
                );
            }
            Err(e) if e.is_normal_close() => {
                println!("\nchannel closed by server — {seen} packet(s) observed");
                break;
            }
            Err(e) => {
                anyhow::bail!("channel recv error after {seen} packet(s): {e}");
            }
        }
    }

    let _ = channel.close().await;
    let _ = client.disconnect().await;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        s.to_string()
    }
}
