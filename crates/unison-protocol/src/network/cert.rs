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
//! | Internal mesh, fixed server↔client pair | [`CertSource::SelfSigned`] via [`InternalMeshKeypair`] |
//! | Internal mesh, many servers (private CA) | [`CertSource::Provided`] via [`super::mesh::MeshCa::issue`] |
//! | Public server (Let's Encrypt etc.) | [`CertSource::Provided`] or [`CertSource::FromFile`] |
//! | Dev quickstart (localhost only) | [`CertSource::dev_localhost`] |
//!
//! # Example
//!
//! ```no_run
//! use unison::network::cert::CertSource;
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
    /// Dev quickstart: minimal self-signed cert for `localhost` + 両 family の loopback。
    ///
    /// **DEV ONLY**: Not suitable for production. The cert has only localhost
    /// SANs and is regenerated every process start. IPv4 (`127.0.0.1`) /
    /// IPv6 (`::1`) 両方を含め、実ホスト名 SNI 検証 (= System trust) でも
    /// dual-stack loopback に接続できるようにする。
    pub fn dev_localhost() -> Self {
        Self::SelfSigned {
            subject_alt_names: vec![
                "localhost".into(),
                "::1".into(),
                "127.0.0.1".into(),
            ],
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

    /// Resolve this source into raw DER material — `(cert chain DER, PKCS#8 key DER)`.
    ///
    /// `wtransport` の [`Identity`](wtransport::Identity) は rustls の
    /// `CertifiedKey` ではなく DER バイト列を要求するため、 WebTransport ingress
    /// (Phase 6a) はこのメソッドで TLS マテリアルを取り出して `cert.rs` と
    /// 信頼モデルを共有する。
    ///
    /// # Limitations
    ///
    /// [`Self::Provided`] は `CertifiedKey` 内に秘密鍵 DER を保持しておらず
    /// (rustls の `SigningKey` は DER を露出しない)、 このメソッドはエラーを
    /// 返す。 WebTransport で in-memory cert を使う場合は [`Self::FromFile`] か
    /// [`Self::SelfSigned`] を使うこと。
    pub fn resolve_der(&self) -> Result<(Vec<Vec<u8>>, Vec<u8>)> {
        match self {
            Self::SelfSigned { subject_alt_names } => {
                // WebTransport ingress 用は **validity を短く** する必要がある。
                // ブラウザの `serverCertificateHashes` pinning は、 pin 対象 cert の
                // 有効期間が 2 週間以内であることを要求する仕様 (= 長寿命の
                // self-signed cert は pin できない)。 rcgen の既定は 1975〜4096 年
                // の超長期なので、 ここでは 13 日有効の cert を生成する。
                let cert_key = generate_webtransport_self_signed(subject_alt_names.clone())?;
                let cert_der = cert_key.cert.der().to_vec();
                let key_der = cert_key.signing_key.serialize_der();
                Ok((vec![cert_der], key_der))
            }
            Self::FromFile {
                cert_path,
                key_path,
            } => {
                let cert_pem = std::fs::read_to_string(cert_path).with_context(|| {
                    format!("failed to read cert file: {}", cert_path.display())
                })?;
                let key_pem_bytes = std::fs::read(key_path)
                    .with_context(|| format!("failed to read key file: {}", key_path.display()))?;

                let certs: Vec<Vec<u8>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .with_context(|| format!("failed to parse cert PEM: {}", cert_path.display()))?
                    .into_iter()
                    .map(|c| c.to_vec())
                    .collect();
                if certs.is_empty() {
                    anyhow::bail!("no certificates found in {}", cert_path.display());
                }

                let key = rustls_pemfile::private_key(&mut key_pem_bytes.as_slice())
                    .with_context(|| format!("failed to parse key PEM: {}", key_path.display()))?
                    .ok_or_else(|| {
                        anyhow::anyhow!("no private key found in {}", key_path.display())
                    })?;
                Ok((certs, key.secret_der().to_vec()))
            }
            Self::Provided { .. } => anyhow::bail!(
                "CertSource::Provided は秘密鍵 DER を保持しないため WebTransport では \
                 使用できません。FromFile または SelfSigned を使ってください"
            ),
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

/// WebTransport `serverCertificateHashes` 互換の自己署名 cert を生成する。
///
/// ブラウザ (および Node polyfill) は cert hash pinning の対象 cert に対し、
/// **有効期間が 2 週間以内**・ECDSA 鍵であることを要求する (WebTransport spec
/// の `serverCertificateHashes` 制約)。 rcgen の既定 cert は validity が
/// 1975〜4096 年と超長期で pin 不可なので、 ここでは:
///
/// - 鍵: ECDSA P-256 (`rcgen::KeyPair::generate` の既定)
/// - validity: `now - 1h` 〜 `now + 13d` (= clock skew を見込んで前倒し、
///   spec の 14 日上限内)
///
/// に明示設定した cert を作る。 dev quickstart 用 (= 短命なので定期再生成前提)。
pub(super) fn generate_webtransport_self_signed(
    sans: Vec<String>,
) -> Result<rcgen::CertifiedKey<rcgen::KeyPair>> {
    use time::{Duration, OffsetDateTime};

    let mut params =
        rcgen::CertificateParams::new(sans).context("rcgen CertificateParams の生成に失敗")?;
    let now = OffsetDateTime::now_utc();
    // clock skew 対策で not_before は 1h 前倒し、 not_after は 13 日後
    // (= spec 上限 14 日に対して余裕を持たせる)。
    params.not_before = now - Duration::hours(1);
    params.not_after = now + Duration::days(13);

    let signing_key = rcgen::KeyPair::generate().context("rcgen 鍵ペアの生成に失敗")?;
    let cert = params
        .self_signed(&signing_key)
        .context("rcgen self-signed cert の署名に失敗")?;
    Ok(rcgen::CertifiedKey { cert, signing_key })
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
