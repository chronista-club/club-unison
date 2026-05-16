//! `unison` — Unison Protocol 開発者 CLI
//!
//! KDL スキーマ駆動の QUIC プロトコルフレームワーク club-unison を、
//! 実サーバなしで叩く・覗く・検証するための dev tool 群。
//!
//! # サブコマンド
//!
//! - `ping <url>`        — サーバへ接続し疎通 + RTT 計測
//! - `sniff <url>`       — channel traffic を覗く packet inspector
//! - `mock --schema F`   — KDL schema から stub server を起動
//! - `schema-lint F`     — KDL schema を parse + invariant 検証

use clap::{Parser, Subcommand};

mod mock;
mod ping;
mod schema_lint;
mod sniff;

/// Unison Protocol developer CLI.
#[derive(Parser)]
#[command(name = "unison", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// サーバへ接続して疎通確認 + ラウンドトリップ遅延を計測する
    Ping(ping::PingArgs),
    /// サーバへ接続し channel traffic を覗く (dev packet inspector)
    Sniff(sniff::SniffArgs),
    /// KDL channel schema から stub 応答する mock server を起動する
    Mock(mock::MockArgs),
    /// KDL schema を parse + invariant 検証する
    SchemaLint(schema_lint::SchemaLintArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // log は stderr へ (= stdout は CLI の primary output に温存)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Ping(args) => ping::run(args).await,
        Command::Sniff(args) => sniff::run(args).await,
        Command::Mock(args) => mock::run(args).await,
        Command::SchemaLint(args) => schema_lint::run(args),
    }
}

/// `--trust` フラグ共通定義 (= ping / sniff で共有)
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum TrustMode {
    /// cert 検証を skip (dev 用、self-signed server 向け)
    Skip,
    /// OS / webpki-roots の trust store を使う (public server 向け)
    System,
}

impl TrustMode {
    /// `unison::network::TrustAnchors` へ変換
    pub fn to_anchors(self) -> unison::network::TrustAnchors {
        match self {
            Self::Skip => unison::network::TrustAnchors::SkipVerification,
            Self::System => unison::network::TrustAnchors::System,
        }
    }
}
