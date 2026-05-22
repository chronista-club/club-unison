//! Trust anchor abstraction for QUIC TLS clients.
//!
//! # Design
//!
//! Mirrors [`crate::network::cert::CertSource`] on the client side. The
//! library does not pick a trust model — the operator does, via one of the
//! [`TrustAnchors`] variants.
//!
//! # Trust quadrant mapping
//!
//! | Scenario | Variant |
//! |----------|---------|
//! | Connect to public server (CA chain) | [`TrustAnchors::System`] |
//! | Internal mesh, fixed pair | [`TrustAnchors::Custom`] (via [`super::mesh::InternalMeshKeypair`]) |
//! | Internal mesh, many servers (private CA) | [`TrustAnchors::Custom`] (via [`super::mesh::MeshCa::trust_anchors`]) |
//! | Dev against `dev_localhost()` server | [`TrustAnchors::SkipVerification`] |

use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};

/// Trust anchors used by the client to verify server certificates.
#[derive(Clone)]
pub enum TrustAnchors {
    /// Trust the OS-native CA bundle (via `webpki-roots`'s Mozilla bundled set).
    ///
    /// Suitable for connecting to public servers whose certs come from a
    /// well-known CA chain.
    System,

    /// Trust only the certs in this list (pinned CAs or self-issued certs).
    ///
    /// Suitable for internal mesh — pair this with
    /// [`super::cert::CertSource::SelfSigned`] on the server side. The helper
    /// [`super::mesh::InternalMeshKeypair::generate`] returns both halves.
    Custom(Vec<CertificateDer<'static>>),

    /// **DEV ONLY** — skip all server certificate verification.
    ///
    /// Suitable for dev quickstart against
    /// [`super::cert::CertSource::dev_localhost`]. A `tracing::warn!` is
    /// emitted on every client build.
    ///
    /// **DO NOT USE IN PRODUCTION**: an attacker on the network path can
    /// impersonate any server.
    SkipVerification,
}

impl TrustAnchors {
    /// Build the underlying [`rustls::ClientConfig`] for this trust mode.
    pub fn build_client_config(self) -> Result<Arc<rustls::ClientConfig>> {
        match self {
            Self::System => {
                let mut roots = RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                let config = rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                Ok(Arc::new(config))
            }
            Self::Custom(certs) => {
                let mut roots = RootCertStore::empty();
                for cert in certs {
                    roots
                        .add(cert)
                        .context("failed to add custom CA certificate to root store")?;
                }
                let config = rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                Ok(Arc::new(config))
            }
            Self::SkipVerification => {
                tracing::warn!(
                    "TrustAnchors::SkipVerification — server certificates will NOT be \
                     verified. DEV / TEST ONLY, never use in production."
                );
                let config = rustls::ClientConfig::builder()
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
                    .with_no_client_auth();
                Ok(Arc::new(config))
            }
        }
    }
}

/// Certificate verifier that accepts every server cert.
///
/// **Used only when [`TrustAnchors::SkipVerification`] is selected.**
#[derive(Debug)]
pub(crate) struct SkipServerVerification;

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA1,
            SignatureScheme::ECDSA_SHA1_Legacy,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
            SignatureScheme::ED448,
        ]
    }
}
