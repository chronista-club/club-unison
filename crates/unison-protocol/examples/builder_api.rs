//! v0.8.0+ Builder API showcase
//!
//! `QuicServer::builder()` / `QuicClient::builder()` を使って **明示的に**
//! cert source / trust anchors を指定する例。
//!
//! ```bash
//! cargo run -p club-unison --example builder_api
//! ```
//!
//! 4 つのユースケースを順に示します:
//! 1. Dev quickstart   (loopback、SelfSigned + SkipVerification)
//! 2. Internal mesh    (server↔server、自己署名 cert + pinned trust)
//! 3. From file        (k8s secret / cert-manager 配置を想定)
//! 4. Public (System)  (Let's Encrypt 等の公開 CA chain を信頼)

use anyhow::Result;
use std::sync::Arc;

use unison::network::quic::{QuicClient, QuicServer};
use unison::network::{CertSource, InternalMeshKeypair, ProtocolServer, TrustAnchors};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    println!("=== v0.8.0 Builder API ===\n");

    // -----------------------------------------------------------------------
    // 1. Dev quickstart — localhost only
    // -----------------------------------------------------------------------
    println!("[1] Dev quickstart (loopback)");
    let _server_quickstart = QuicServer::builder(Arc::new(ProtocolServer::new()))
        .cert_source(CertSource::dev_localhost())
        .build();
    let _client_quickstart = QuicClient::builder()
        .trust_anchors(TrustAnchors::SkipVerification) // dev だけ、本番では使わない
        .build()?;
    println!("    server: CertSource::dev_localhost()");
    println!("    client: TrustAnchors::SkipVerification\n");

    // -----------------------------------------------------------------------
    // 2. Internal mesh — server↔server with shared cert material
    // -----------------------------------------------------------------------
    println!("[2] Internal mesh (server↔server, pinned trust)");
    let mesh = InternalMeshKeypair::generate([
        "broker.local".into(),
        "*.unison.svc.cluster.local".into(),
    ])?;
    let _server_mesh = QuicServer::builder(Arc::new(ProtocolServer::new()))
        .cert_source(mesh.server_cert_source)
        .build();
    let _client_mesh = QuicClient::builder()
        .trust_anchors(mesh.client_trust_anchors)
        .build()?;
    println!("    InternalMeshKeypair で server cert + client trust を pair 生成");
    println!("    両端が同じ cert material に bind され、SkipVerification 不要\n");

    // -----------------------------------------------------------------------
    // 3. Production via filesystem (k8s secret mount)
    // -----------------------------------------------------------------------
    println!("[3] Production (from file)");
    println!("    server: CertSource::FromFile {{ cert_path: '/etc/tls/tls.crt', ... }}");
    println!("    client: TrustAnchors::System (or TrustAnchors::Custom(ca_chain))\n");
    // 実ファイルが無いと build に失敗するので例示のみ:
    // let server = QuicServer::builder(Arc::new(ProtocolServer::new()))
    //     .cert_source(CertSource::FromFile {
    //         cert_path: "/etc/tls/tls.crt".into(),
    //         key_path: "/etc/tls/tls.key".into(),
    //     })
    //     .build();

    // -----------------------------------------------------------------------
    // 4. Public CA chain — connect to a public Unison gateway
    // -----------------------------------------------------------------------
    println!("[4] Public CA chain (system trust)");
    let _client_public = QuicClient::builder()
        .trust_anchors(TrustAnchors::System)
        .build()?;
    println!("    client: TrustAnchors::System (webpki-roots Mozilla bundle)\n");

    println!("=== 全パターン configure OK ===");
    Ok(())
}
