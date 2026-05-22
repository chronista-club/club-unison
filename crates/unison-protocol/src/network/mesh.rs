//! Internal mesh trust helpers.
//!
//! Two primitives for establishing TLS trust between Unison endpoints under a
//! single operator's control, **without a public CA**:
//!
//! - [`InternalMeshKeypair`] — one self-signed cert shared by a server and its
//!   client. Simplest; fits a single fixed pair (e.g. broker ↔ TUI).
//! - [`MeshCa`] — a private certificate authority that signs a per-server leaf
//!   cert. Scales to many servers: clients trust only the CA, and adding a
//!   server needs no client-side change.
//!
//! Both pair with [`CertSource`] on the server and [`TrustAnchors`] on the
//! client — the library still does not pick a trust model, it just makes the
//! mesh quadrant ergonomic.

use std::sync::Arc;

use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use time::{Duration, OffsetDateTime};

use super::cert::{CertSource, generate_self_signed_with_der};
use super::trust::TrustAnchors;

/// Paired server cert + client trust anchor for internal mesh communication.
///
/// Both halves are derived from a single freshly-generated self-signed cert,
/// so the client's [`TrustAnchors::Custom`] contains exactly the cert that
/// the server presents.
///
/// # When to use
///
/// Fits exactly one server bound to its client(s) by a shared cert (e.g. HG
/// broker ↔ Heaven's Door TUI). For a mesh of many servers use [`MeshCa`]
/// instead — sharing one `InternalMeshKeypair` across N servers means sharing
/// one private key (no per-server compromise isolation), and generating N of
/// them forces every client onto an O(N) trust list.
///
/// # Example
///
/// ```no_run
/// use unison::network::mesh::InternalMeshKeypair;
///
/// let pair = InternalMeshKeypair::generate(
///     ["broker.local".into(), "*.unison.svc.cluster.local".into()],
/// )?;
///
/// // Server uses `pair.server_cert_source`
/// // Client uses `pair.client_trust_anchors`
/// # Ok::<_, anyhow::Error>(())
/// ```
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

/// A private certificate authority for an internal Unison mesh.
///
/// Where [`InternalMeshKeypair`] binds one server to its client by a shared
/// self-signed cert, `MeshCa` scales to many servers: one CA signs a
/// per-server leaf cert, and every client trusts just the CA.
///
/// - **O(1) client trust** — clients hold only the CA cert (via
///   [`trust_anchors`](Self::trust_anchors)); a new server needs no client
///   change.
/// - **per-server keys** — each [`issue`](Self::issue) call mints a fresh leaf
///   key, so one compromised server does not expose the others.
/// - **rotation** — re-issue a leaf; the CA is unchanged.
///
/// Client-side this needs **no new code**: [`trust_anchors`](Self::trust_anchors)
/// returns [`TrustAnchors::Custom`], whose `RootCertStore` lets rustls
/// chain-verify any leaf the CA signed.
///
/// The CA is persisted as PEM ([`to_pem`](Self::to_pem) /
/// [`from_pem`](Self::from_pem)). The CA private key can mint a certificate
/// for any mesh identity — guard it like any other root secret.
///
/// # Example
///
/// ```no_run
/// use unison::network::mesh::MeshCa;
///
/// // Control plane: generate once, persist the PEM securely.
/// let ca = MeshCa::generate()?;
/// let (ca_cert_pem, ca_key_pem) = ca.to_pem();
///
/// // Server side: issue a leaf cert for this server's identity.
/// let server_cert = ca.issue(["cp.fleetstage.cloud".into()])?;
///
/// // Client side: trust just the CA — unchanged as servers come and go.
/// let client_trust = ca.trust_anchors();
/// # Ok::<_, anyhow::Error>(())
/// ```
pub struct MeshCa {
    /// Signs leaf certs and carries the CA's distinguished name / key usages.
    issuer: Issuer<'static, KeyPair>,
    /// CA cert DER — for client trust anchors and issued-leaf chains.
    ca_cert_der: CertificateDer<'static>,
    /// CA cert PEM — retained so [`to_pem`](Self::to_pem) needs no re-encode.
    ca_cert_pem: String,
}

impl MeshCa {
    /// CA certificate validity (days). The CA outlives many leaf rotations.
    const CA_VALIDITY_DAYS: i64 = 3650;
    /// Default issued-leaf validity (days). Rotate by re-issuing.
    const LEAF_VALIDITY_DAYS: i64 = 90;

    /// Generate a fresh mesh CA — a new key plus a self-signed CA certificate.
    pub fn generate() -> Result<Self> {
        let mut params = CertificateParams::new(Vec::<String>::new())
            .context("MeshCa: building CA certificate params")?;
        // A real CA: BasicConstraints CA:TRUE + keyCertSign so rustls accepts
        // it as an issuer and chain-verifies leaves against it.
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params
            .distinguished_name
            .push(DnType::CommonName, "Unison Mesh CA");
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::hours(1);
        params.not_after = now + Duration::days(Self::CA_VALIDITY_DAYS);

        let ca_key = KeyPair::generate().context("MeshCa: generating CA key")?;
        let ca_cert = params
            .self_signed(&ca_key)
            .context("MeshCa: self-signing CA certificate")?;
        let ca_cert_der = ca_cert.der().clone();
        let ca_cert_pem = ca_cert.pem();
        let issuer = Issuer::new(params, ca_key);

        Ok(Self {
            issuer,
            ca_cert_der,
            ca_cert_pem,
        })
    }

    /// Load a CA previously serialized with [`to_pem`](Self::to_pem).
    pub fn from_pem(ca_cert_pem: &str, ca_key_pem: &str) -> Result<Self> {
        let ca_key = KeyPair::from_pem(ca_key_pem).context("MeshCa: parsing CA key PEM")?;
        let issuer = Issuer::from_ca_cert_pem(ca_cert_pem, ca_key)
            .context("MeshCa: parsing CA certificate PEM")?;
        // `Issuer` は CA cert DER を保持しない (rcgen の設計) ため、rustls 用の
        // trust anchor / leaf chain に渡す DER は PEM から独立に取り出す。
        let ca_cert_der = rustls_pemfile::certs(&mut ca_cert_pem.as_bytes())
            .next()
            .context("MeshCa: no certificate found in CA PEM")?
            .context("MeshCa: parsing CA certificate PEM")?;

        Ok(Self {
            issuer,
            ca_cert_der,
            ca_cert_pem: ca_cert_pem.to_string(),
        })
    }

    /// Serialize the CA as `(cert PEM, key PEM)` for persistence.
    ///
    /// **The key PEM can mint a certificate for any mesh identity** — store it
    /// with the same care as any root credential (restricted perms, secret
    /// manager). The cert PEM is public.
    pub fn to_pem(&self) -> (String, String) {
        (self.ca_cert_pem.clone(), self.issuer.key().serialize_pem())
    }

    /// Issue a CA-signed leaf certificate for `sans`, as a server [`CertSource`].
    ///
    /// The returned [`CertSource::Provided`] carries the `[leaf, CA]` chain and
    /// a fresh per-leaf key. A client that trusts this CA (via
    /// [`trust_anchors`](Self::trust_anchors)) chain-verifies the leaf with no
    /// extra configuration. Leaf validity defaults to 90 days — rotate by
    /// re-issuing.
    pub fn issue(&self, sans: impl IntoIterator<Item = String>) -> Result<CertSource> {
        let sans: Vec<String> = sans.into_iter().collect();
        let mut params =
            CertificateParams::new(sans).context("MeshCa: building leaf certificate params")?;
        // A leaf, explicitly not a CA. `ServerAuth` EKU is required: under
        // `TrustAnchors::Custom([ca])` rustls runs its webpki verifier, which
        // checks the leaf carries the serverAuth extended key usage.
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - Duration::hours(1);
        params.not_after = now + Duration::days(Self::LEAF_VALIDITY_DAYS);

        let leaf_key = KeyPair::generate().context("MeshCa: generating leaf key")?;
        let leaf_cert = params
            .signed_by(&leaf_key, &self.issuer)
            .context("MeshCa: signing leaf certificate")?;

        let leaf_der = leaf_cert.der().clone();
        let key_der = leaf_key.serialize_der();
        let private_key = PrivateKeyDer::try_from(key_der)
            .map_err(|e| anyhow::anyhow!("MeshCa: leaf key parse: {e}"))?;
        let signing_key = any_supported_type(&private_key)
            .map_err(|e| anyhow::anyhow!("MeshCa: leaf signing key build: {e}"))?;

        // Present [leaf, CA] as the chain. With `trust_anchors()` the CA is the
        // client's root; shipping the chain is standard TLS practice.
        let chain = vec![leaf_der, self.ca_cert_der.clone()];
        Ok(CertSource::Provided {
            certified_key: Arc::new(CertifiedKey::new(chain, signing_key)),
        })
    }

    /// The client-side trust anchor for this CA — `Custom([CA cert])`.
    ///
    /// Every client in the mesh uses this; it does **not** change when servers
    /// are added or their leaf certs rotate.
    pub fn trust_anchors(&self) -> TrustAnchors {
        TrustAnchors::Custom(vec![self.ca_cert_der.clone()])
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustls::RootCertStore;
    use rustls::client::WebPkiServerVerifier;
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};

    use super::*;

    /// `issue()` が返す `CertSource::Provided` から `[leaf, ca]` チェーンを取り出す。
    fn chain_of(src: &CertSource) -> Vec<CertificateDer<'static>> {
        match src {
            CertSource::Provided { certified_key } => certified_key.cert.clone(),
            _ => panic!("MeshCa::issue must return CertSource::Provided"),
        }
    }

    /// `MeshCa` の CA cert を 1 つだけ含む `RootCertStore` から検証器を作る。
    fn verifier_trusting(ca: &MeshCa) -> Arc<WebPkiServerVerifier> {
        let TrustAnchors::Custom(ca_certs) = ca.trust_anchors() else {
            panic!("trust_anchors() must be Custom");
        };
        let mut roots = RootCertStore::empty();
        roots
            .add(ca_certs[0].clone())
            .expect("add CA to root store");
        WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .expect("build WebPkiServerVerifier")
    }

    #[test]
    fn generate_succeeds() {
        MeshCa::generate().expect("MeshCa::generate");
    }

    #[test]
    fn pem_roundtrip_preserves_the_ca() {
        let ca = MeshCa::generate().unwrap();
        let (cert_pem, key_pem) = ca.to_pem();
        let reloaded = MeshCa::from_pem(&cert_pem, &key_pem).expect("from_pem");

        let TrustAnchors::Custom(orig) = ca.trust_anchors() else {
            unreachable!()
        };
        let TrustAnchors::Custom(back) = reloaded.trust_anchors() else {
            unreachable!()
        };
        assert_eq!(orig, back, "reloaded CA must point at the same cert");
    }

    #[test]
    fn issue_returns_a_leaf_ca_chain() {
        let ca = MeshCa::generate().unwrap();
        let chain = chain_of(&ca.issue(["leaf.test".to_string()]).expect("issue"));

        assert_eq!(chain.len(), 2, "chain should be [leaf, ca]");
        let TrustAnchors::Custom(ca_certs) = ca.trust_anchors() else {
            unreachable!()
        };
        assert_eq!(chain[1], ca_certs[0], "chain[1] must be the CA cert");
    }

    #[test]
    fn issued_leaf_verifies_against_its_ca() {
        let ca = MeshCa::generate().unwrap();
        let chain = chain_of(&ca.issue(["leaf.test".to_string()]).unwrap());
        let verifier = verifier_trusting(&ca);

        verifier
            .verify_server_cert(
                &chain[0],
                &chain[1..],
                &ServerName::try_from("leaf.test").unwrap(),
                &[],
                UnixTime::now(),
            )
            .expect("a CA-signed leaf must verify against that CA's trust anchor");
    }

    #[test]
    fn leaf_is_rejected_by_an_unrelated_ca() {
        let ca_a = MeshCa::generate().unwrap();
        let ca_b = MeshCa::generate().unwrap();
        let chain = chain_of(&ca_a.issue(["leaf.test".to_string()]).unwrap());

        // CA-B の trust anchor で CA-A 発行の leaf を検証 → 失敗するはず。
        let result = verifier_trusting(&ca_b).verify_server_cert(
            &chain[0],
            &chain[1..],
            &ServerName::try_from("leaf.test").unwrap(),
            &[],
            UnixTime::now(),
        );
        assert!(
            result.is_err(),
            "a leaf signed by an unrelated CA must not verify"
        );
    }

    #[test]
    fn reloaded_ca_still_issues_verifiable_leaves() {
        let ca = MeshCa::generate().unwrap();
        let (cert_pem, key_pem) = ca.to_pem();
        let reloaded = MeshCa::from_pem(&cert_pem, &key_pem).unwrap();

        // from_pem 後の CA が発行した leaf も、元 CA の trust anchor で検証できる。
        let chain = chain_of(&reloaded.issue(["leaf.test".to_string()]).unwrap());
        verifier_trusting(&ca)
            .verify_server_cert(
                &chain[0],
                &chain[1..],
                &ServerName::try_from("leaf.test").unwrap(),
                &[],
                UnixTime::now(),
            )
            .expect("leaf from a reloaded CA must verify against the original trust anchor");
    }
}
