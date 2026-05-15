//! Certificate source abstraction for QUIC TLS servers.
//!
//! # Design
//!
//! The library does **not** pick a TLS trust model — the operator does, via
//! one of the [`CertSource`] variants below. See the project design memory
//! "Unison TLS 証明書アーキテクチャ設計" for the full rationale and 4-quadrant
//! analysis.
//!
//! # Trust quadrant mapping
//!
//! | Scenario | Variant |
//! |----------|---------|
//! | Internal cluster mesh (server↔server) | [`CertSource::SelfSigned`] via [`InternalMeshKeypair`] |
//! | Public server (Let's Encrypt etc.) | [`CertSource::Provided`] or [`CertSource::FromFile`] |
//! | Dev quickstart (localhost only) | [`CertSource::dev_localhost`] |
//!
//! # Example
//!
//! ```no_run
//! use club_unison::network::cert::CertSource;
//!
//! // Dev quickstart
//! let source = CertSource::dev_localhost();
//!
//! // K8s secret mount
//! let source = CertSource::FromFile {
//!     cert_path: "/etc/tls/tls.crt".into(),
//!     key_path: "/etc/tls/tls.key".into(),
//! };
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;

/// Certificate acquisition strategy for a Unison server.
#[derive(Clone)]
pub enum CertSource {
    /// Generate a self-signed certificate at startup with the given SANs.
    ///
    /// Suitable for: dev quickstart, internal cluster mesh, ephemeral sessions.
    /// **Not suitable for** public-facing servers — clients without the matching
    /// trust anchor will reject the cert.
    SelfSigned { subject_alt_names: Vec<String> },

    /// Provide cert chain + signing key directly (in-memory).
    ///
    /// Uses [`Arc<CertifiedKey>`] to avoid private key duplication
    /// (rustls's `PrivateKeyDer` does not implement `Zeroize`).
    Provided { certified_key: Arc<CertifiedKey> },

    /// Load cert + key from filesystem paths at startup.
    ///
    /// Suitable for: k8s secret volume mounts, cert-manager output, classical
    /// PKI deployments.
    FromFile {
        cert_path: PathBuf,
        key_path: PathBuf,
    },
}

impl CertSource {
    /// Dev quickstart: minimal self-signed cert for `localhost` + `[::1]`.
    ///
    /// **DEV ONLY**: Not suitable for production. The cert has only localhost
    /// SANs and is regenerated every process start.
    pub fn dev_localhost() -> Self {
        Self::SelfSigned {
            subject_alt_names: vec!["localhost".into(), "::1".into()],
        }
    }

    /// Internal cluster mesh: self-signed for given SANs.
    ///
    /// For the matching client-side trust anchor, use
    /// [`super::mesh::InternalMeshKeypair::generate`] which returns both the
    /// server's `CertSource` and the client's `TrustAnchors::Custom`.
    pub fn internal_mesh(sans: impl IntoIterator<Item = String>) -> Self {
        Self::SelfSigned {
            subject_alt_names: sans.into_iter().collect(),
        }
    }

    /// Resolve this source into a usable [`CertifiedKey`].
    ///
    /// I/O timing:
    /// - [`Self::SelfSigned`]: generates in-memory (no disk I/O)
    /// - [`Self::Provided`]: returns the Arc directly (no work)
    /// - [`Self::FromFile`]: reads files synchronously (blocks the current task briefly)
    pub fn resolve(self) -> Result<Arc<CertifiedKey>> {
        match self {
            Self::SelfSigned { subject_alt_names } => generate_self_signed(subject_alt_names),
            Self::Provided { certified_key } => Ok(certified_key),
            Self::FromFile {
                cert_path,
                key_path,
            } => load_from_files(&cert_path, &key_path),
        }
    }
}

/// Generate a self-signed cert + key pair and wrap as [`CertifiedKey`].
///
/// Also returns the raw cert DER for callers that need to derive a trust
/// anchor from the same material (e.g., [`InternalMeshKeypair`]).
pub(super) fn generate_self_signed_with_der(
    sans: Vec<String>,
) -> Result<(Arc<CertifiedKey>, CertificateDer<'static>)> {
    let cert_key = rcgen::generate_simple_self_signed(sans)
        .context("rcgen failed to generate self-signed certificate")?;
    let cert_der_bytes = cert_key.cert.der().to_vec();
    let key_der_bytes = cert_key.signing_key.serialize_der();

    let cert_der = CertificateDer::from(cert_der_bytes);
    let certs = vec![cert_der.clone()];
    let private_key = PrivateKeyDer::try_from(key_der_bytes)
        .map_err(|e| anyhow::anyhow!("self-signed key parse: {}", e))?;
    let signing_key = any_supported_type(&private_key)
        .map_err(|e| anyhow::anyhow!("rustls signing_key build: {}", e))?;

    Ok((Arc::new(CertifiedKey::new(certs, signing_key)), cert_der))
}

fn generate_self_signed(sans: Vec<String>) -> Result<Arc<CertifiedKey>> {
    generate_self_signed_with_der(sans).map(|(key, _)| key)
}

fn load_from_files(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<Arc<CertifiedKey>> {
    let cert_pem = std::fs::read_to_string(cert_path)
        .with_context(|| format!("failed to read cert file: {}", cert_path.display()))?;
    let key_pem_bytes = std::fs::read(key_path)
        .with_context(|| format!("failed to read key file: {}", key_path.display()))?;

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse cert PEM: {}", cert_path.display()))?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", cert_path.display());
    }

    let private_key = rustls_pemfile::private_key(&mut key_pem_bytes.as_slice())
        .with_context(|| format!("failed to parse key PEM: {}", key_path.display()))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path.display()))?;

    let signing_key = any_supported_type(&private_key)
        .map_err(|e| anyhow::anyhow!("rustls signing_key build: {}", e))?;

    Ok(Arc::new(CertifiedKey::new(certs, signing_key)))
}
