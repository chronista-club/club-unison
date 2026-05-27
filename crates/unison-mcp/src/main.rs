//! unison-mcp — MCP bridge for the Unison Protocol.
//!
//! Stdio MCP server that exposes 3 tools to AI agents:
//!
//! - `unison_ping` — endpoint への疎通確認 (= probe 互換 escape hatch)
//! - `unison_call` — 任意 channel / method に payload を送信 (= probe 互換 escape hatch)
//! - `unison_discover` — `unison.discovery` channel 経由で server の protocol KDL を
//!   fetch し、 channel / request 一覧 + schema 情報を返す (= NEW、 Hailing α P2-Rust
//!   の `DynamicProtocol::fetch` を MCP tool として exposure)
//!
//! 起動: `unison-mcp [--config <path>]`
//!
//! `--config` を omit すると default config で動作 (= endpoint 未設定、 全 tool に
//! endpoint arg を渡す必要)。 `--config` を指定すると `BridgeConfig` の default
//! endpoint / trust が tool で省略可能になる。
//!
//! 後継位置づけ: `unison-mcp-probe` の superset (= probe の `unison_channel_list`
//! TODO を `unison_discover` が埋める)。 probe は Hailing α P3c で deletion 予定。

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

mod bridge;
mod config;
mod tools;

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

    let bridge = bridge::UnisonBridge::new(config);
    let server = tools::UnisonMcp::new(bridge)
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("MCP serve error: {:?}", e))?;

    server.waiting().await?;
    Ok(())
}
