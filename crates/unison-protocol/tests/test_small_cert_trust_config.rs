//! Small: cert / trust 設定が有効な rustls config を組めることの smoke test。
//!
//! [`CertSource`] / [`TrustAnchors`] から `ServerConfig` / `ClientConfig` が
//! 構築できるところまでを見る。 **TLS handshake も QUIC 通信も行わない** ので
//! ネットワークは不要 (= Small、 常時実行)。 実際の handshake は
//! `test_medium_*` 系が実 QUIC connection で網羅する。
//!
//! (旧 `simple_quic_test.rs`。 QUIC 通信をしないのに名前が QUIC を名乗り、
//! さらに 4 本のうち 3 本が `#[test]` の付かないヘルパーで、 1 本の
//! `test_simple_quic_functionality` から順に呼ばれていた。 最初の assert で
//! 止まるため後続が走らず、 どれが落ちたかも runner から分からなかったので、
//! それぞれ独立した `#[tokio::test]` に分けた。)

use anyhow::Result;

use unison::network::quic::{QuicClient, QuicServer};
use unison::network::{CertSource, InternalMeshKeypair, TrustAnchors};

/// `CertSource::dev_localhost()` から server config が組める。
#[tokio::test]
async fn dev_localhost_cert_builds_server_config() -> Result<()> {
    let result = QuicServer::configure_server_with(CertSource::dev_localhost()).await;
    assert!(
        result.is_ok(),
        "configure_server_with(dev_localhost) should succeed: {:?}",
        result.err()
    );
    Ok(())
}

/// `TrustAnchors::SkipVerification` から client config が組める。
#[tokio::test]
async fn skip_verification_builds_client_config() -> Result<()> {
    let result = QuicClient::configure_client_with(TrustAnchors::SkipVerification).await;
    assert!(
        result.is_ok(),
        "configure_client_with(SkipVerification) should succeed: {:?}",
        result.err()
    );
    Ok(())
}

/// `TrustAnchors::System` は webpki-roots の Mozilla bundle で config を組める。
#[tokio::test]
async fn system_trust_anchors_build_client_config() -> Result<()> {
    let result = QuicClient::configure_client_with(TrustAnchors::System).await;
    assert!(
        result.is_ok(),
        "TrustAnchors::System should build successfully: {:?}",
        result.err()
    );
    Ok(())
}

/// `InternalMeshKeypair` の両側 (server cert / client trust) が config になる。
#[tokio::test]
async fn internal_mesh_keypair_builds_both_sides() -> Result<()> {
    let pair = InternalMeshKeypair::generate(["broker.test".to_string(), "::1".to_string()])?;

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
    Ok(())
}
