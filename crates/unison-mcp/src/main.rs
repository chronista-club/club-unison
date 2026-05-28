//! unison-mcp — MCP bridge for the Unison Protocol.
//!
//! Stdio MCP server that exposes 3 tools to AI agents:
//!
//! Static escape hatch tools:
//! - `unison_ping` — endpoint への疎通確認
//! - `unison_call` — 任意 channel / method に payload を送信 (= generic、 schema 検証なし)
//! - `unison_discover` — `unison.discovery` channel 経由で server の protocol KDL を
//!   fetch し、 channel / request 一覧 + schema 情報を返す
//!
//! Config endpoint がある場合は、 起動時に `DynamicProtocol::fetch` で discovery が走り、
//! channel.request 毎に `unison_<channel>_<method>` 形式の **typed synthesized tools** が
//! 動的に exposure される (= AI agent が初見の Unison server を typed call できる)。
//!
//! 起動: `unison-mcp [--config <path>]`
//!
//! `--config` を omit すると default config で動作 (= endpoint 未設定、 static escape
//! hatch tools のみ、 全 tool に endpoint arg を渡す必要)。 `--config` を指定すると
//! `BridgeConfig` の default endpoint / trust が tool で省略可能 + synthesized typed tools
//! が available。

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use unison_mcp::{bridge, config, tools};

#[derive(Parser, Debug)]
#[command(name = "unison-mcp", version, about, long_about = None)]
struct Args {
    /// Optional bridge config (= default endpoint + trust mode)
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // stdout は MCP transport に使うので、 log は stderr に出す
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let config = match &args.config {
        Some(path) => config::BridgeConfig::from_file(path)
            .with_context(|| format!("failed to load config: {}", path.display()))?,
        None => config::BridgeConfig::default(),
    };
    tracing::info!("unison-mcp starting on stdio (config={:?})", config);

    let bridge = bridge::UnisonBridge::new(config)
        .await
        .context("bridge initialization failed")?;
    let server = tools::UnisonMcp::new(bridge)
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("MCP serve error: {:?}", e))?;

    server.waiting().await?;
    Ok(())
}
