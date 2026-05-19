//! `unison call <url> --channel <name> --method <name>` — channel に request を
//! 1 本送り、response を表示する dev tool。
//!
//! `mock`（サーバ側の stub 応答）と対になるクライアント側コマンド。channel を
//! open し、typed request を 1 回送って response payload を JSON で stdout に
//! 出して終了する。`mock` を別ターミナルで起動すれば、CLI だけで
//! request/response ループを閉じられる。

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use unison::ProtocolClient;
use unison::network::quic::QuicClient;

use crate::TrustMode;

#[derive(Args)]
pub struct CallArgs {
    /// Unison サーバの URL (例: `quic://[::1]:7878`)
    pub url: String,

    /// request を送る channel 名
    #[arg(short, long)]
    pub channel: String,

    /// 呼び出す request method 名
    #[arg(short, long)]
    pub method: String,

    /// request payload (JSON 文字列、省略時は空 object `{}`)
    #[arg(short, long, default_value = "{}")]
    pub payload: String,

    /// trust anchor mode (default: skip)
    #[arg(long, value_enum, default_value = "skip")]
    pub trust: TrustMode,

    /// response 待ちタイムアウト (ミリ秒、省略時は channel 既定値)
    #[arg(long)]
    pub timeout: Option<u64>,
}

pub async fn run(args: CallArgs) -> Result<()> {
    // payload を JSON として検証 (= 不正なら connect 前に即エラー)
    let payload: serde_json::Value = serde_json::from_str(&args.payload)
        .with_context(|| format!("--payload is not valid JSON: {}", args.payload))?;

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

    let channel = {
        let ch = client
            .open_channel(&args.channel)
            .await
            .context("open_channel failed")?;
        match args.timeout {
            Some(ms) => ch.with_request_timeout(Duration::from_millis(ms)),
            None => ch,
        }
    };

    eprintln!("→ request '{}'", args.method);
    let response: serde_json::Value = channel
        .request(&args.method, &payload)
        .await
        .context("request failed")?;

    // response payload を pretty JSON で stdout に出す (= CLI の primary output)
    println!("{}", serde_json::to_string_pretty(&response)?);

    let _ = channel.close().await;
    let _ = client.disconnect().await;
    Ok(())
}
