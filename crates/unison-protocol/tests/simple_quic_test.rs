//! Smoke tests for the v0.7.0 cert / trust API.
//!
//! These tests confirm that the new `CertSource` / `TrustAnchors` plumbing
//! produces a working `ServerConfig` / `ClientConfig`. They do not exercise
//! the actual TLS handshake (covered by integration tests).

use anyhow::Result;
use tracing::{Level, info};

#[tokio::test]
async fn test_simple_quic_functionality() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_test_writer()
        .try_init()
        .ok();

    info!("🧪 Running simple QUIC functionality test");

    test_quic_server_config_with_dev_localhost().await?;
    test_quic_client_config_with_skip_verification().await?;
    test_internal_mesh_pair_generates().await?;

    info!("✅ All simple QUIC tests passed!");
    Ok(())
}

async fn test_quic_server_config_with_dev_localhost() -> Result<()> {
    use club_unison::network::CertSource;
    use club_unison::network::quic::QuicServer;

    info!("🔧 Testing QUIC server configuration with CertSource::dev_localhost()");
    let result = QuicServer::configure_server_with(CertSource::dev_localhost()).await;
    assert!(
        result.is_ok(),
        "configure_server_with(dev_localhost) should succeed: {:?}",
        result.err()
    );
    info!("✅ QUIC server configuration test passed");
    Ok(())
}

async fn test_quic_client_config_with_skip_verification() -> Result<()> {
    use club_unison::network::TrustAnchors;
    use club_unison::network::quic::QuicClient;

    info!("🔧 Testing QUIC client configuration with TrustAnchors::SkipVerification");
    let result = QuicClient::configure_client_with(TrustAnchors::SkipVerification).await;
    assert!(
        result.is_ok(),
        "configure_client_with(SkipVerification) should succeed: {:?}",
        result.err()
    );
    info!("✅ QUIC client configuration test passed");
    Ok(())
}

async fn test_internal_mesh_pair_generates() -> Result<()> {
    use club_unison::network::InternalMeshKeypair;
    use club_unison::network::quic::{QuicClient, QuicServer};

    info!("🔐 Testing InternalMeshKeypair pair generation");
    let pair = InternalMeshKeypair::generate(["broker.test".to_string(), "::1".to_string()])?;

    // Both halves should produce valid configs
    let server_result = QuicServer::configure_server_with(pair.server_cert_source).await;
    assert!(
        server_result.is_ok(),
        "server side from mesh keypair should configure: {:?}",
        server_result.err()
    );

    let client_result = QuicClient::configure_client_with(pair.client_trust_anchors).await;
    assert!(
        client_result.is_ok(),
        "client side from mesh keypair should configure: {:?}",
        client_result.err()
    );

    info!("✅ InternalMeshKeypair pair test passed");
    Ok(())
}

/// `TrustAnchors::System` should produce a config using webpki-roots Mozilla bundle.
#[tokio::test]
async fn test_trust_anchors_system_builds() -> Result<()> {
    use club_unison::network::TrustAnchors;
    use club_unison::network::quic::QuicClient;

    let result = QuicClient::configure_client_with(TrustAnchors::System).await;
    assert!(
        result.is_ok(),
        "TrustAnchors::System should build successfully: {:?}",
        result.err()
    );
    Ok(())
}
