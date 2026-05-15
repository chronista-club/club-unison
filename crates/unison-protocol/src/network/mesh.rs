//! Internal mesh helper: paired server cert + client trust anchor.
//!
//! # Why a pair?
//!
//! When two Unison endpoints under the same operator's control want to
//! communicate (e.g., HG broker ↔ Heaven's Door TUI within Chronista),
//! they need:
//! - **Server side**: a self-signed cert with the appropriate SANs
//! - **Client side**: a trust anchor that recognises that exact cert
//!
//! [`InternalMeshKeypair::generate`] returns both halves derived from the
//! same self-signed certificate, eliminating the need for the client to fall
//! back to [`crate::network::trust::TrustAnchors::SkipVerification`].
//!
//! # Example
//!
//! ```no_run
//! use club_unison::network::mesh::InternalMeshKeypair;
//!
//! let pair = InternalMeshKeypair::generate(
//!     ["broker.local".into(), "*.unison.svc.cluster.local".into()],
//! )?;
//!
//! // Server uses `pair.server_cert_source`
//! // Client uses `pair.client_trust_anchors`
//! # Ok::<_, anyhow::Error>(())
//! ```

use std::sync::Arc;

use anyhow::Result;

use super::cert::{CertSource, generate_self_signed_with_der};
use super::trust::TrustAnchors;

/// Paired server cert + client trust anchor for internal mesh communication.
///
/// Both halves are derived from a single freshly-generated self-signed cert,
/// so the client's [`TrustAnchors::Custom`] contains exactly the cert that
/// the server presents.
pub struct InternalMeshKeypair {
    /// Server-side: feed this into [`crate::network::ProtocolServer`]'s builder.
    pub server_cert_source: CertSource,
    /// Client-side: feed this into [`crate::network::ProtocolClient`]'s builder.
    pub client_trust_anchors: TrustAnchors,
}

impl InternalMeshKeypair {
    /// Generate a fresh self-signed cert for the given SANs, returning both
    /// the server `CertSource` and the matching client `TrustAnchors`.
    ///
    /// Each call generates a new key — the pair is bound by the cert material.
    pub fn generate(sans: impl IntoIterator<Item = String>) -> Result<Self> {
        let sans: Vec<String> = sans.into_iter().collect();
        let (certified_key, cert_der) = generate_self_signed_with_der(sans)?;

        Ok(Self {
            server_cert_source: CertSource::Provided {
                certified_key: Arc::clone(&certified_key),
            },
            client_trust_anchors: TrustAnchors::Custom(vec![cert_der]),
        })
    }
}
