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
use rustls::pki_types::pem::{self, PemObject};
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
            subject_alt_names: vec!["localhost".into(), "::1".into(), "127.0.0.1".into()],
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

                let certs = certs_from_pem(&cert_pem, cert_path)?
                    .into_iter()
                    .map(|c| c.to_vec())
                    .collect();
                let key = private_key_from_pem(&key_pem_bytes, key_path)?;
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

/// PEM の CERTIFICATE section を全部読む。 1 つも無ければ error。
///
/// `pem::Error` の `std::error::Error` 実装は pki-types の `std` feature 付きでしか
/// 入らないので、 `Display` だけで anyhow に変換する。
fn certs_from_pem(cert_pem: &str, path: &std::path::Path) -> Result<Vec<CertificateDer<'static>>> {
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("failed to parse cert PEM: {}: {e}", path.display()))?;
    if certs.is_empty() {
        anyhow::bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

/// PEM から最初の秘密鍵 (PKCS#1 / PKCS#8 / SEC1) を読む。
fn private_key_from_pem(key_pem: &[u8], path: &std::path::Path) -> Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| match e {
        pem::Error::NoItemsFound => anyhow::anyhow!("no private key found in {}", path.display()),
        e => anyhow::anyhow!("failed to parse key PEM: {}: {e}", path.display()),
    })
}

fn load_from_files(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> Result<Arc<CertifiedKey>> {
    let cert_pem = std::fs::read_to_string(cert_path)
        .with_context(|| format!("failed to read cert file: {}", cert_path.display()))?;
    let key_pem_bytes = std::fs::read(key_path)
        .with_context(|| format!("failed to read key file: {}", key_path.display()))?;

    let certs = certs_from_pem(&cert_pem, cert_path)?;
    let private_key = private_key_from_pem(&key_pem_bytes, key_path)?;

    let signing_key = any_supported_type(&private_key)
        .map_err(|e| anyhow::anyhow!("rustls signing_key build: {}", e))?;

    Ok(Arc::new(CertifiedKey::new(certs, signing_key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// rcgen で作った `(cert PEM, key PEM, cert DER, key DER)`
    fn generated_pem() -> (String, String, Vec<u8>, Vec<u8>) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        (
            ck.cert.pem(),
            ck.signing_key.serialize_pem(),
            ck.cert.der().to_vec(),
            ck.signing_key.serialize_der(),
        )
    }

    /// `dir` に cert / key を書き、 それを指す `FromFile` を返す
    fn from_file(dir: &Path, cert_pem: &str, key_pem: &str) -> CertSource {
        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");
        std::fs::write(&cert_path, cert_pem).unwrap();
        std::fs::write(&key_path, key_pem).unwrap();
        CertSource::FromFile {
            cert_path,
            key_path,
        }
    }

    #[test]
    fn from_file_loads_cert_and_key_pem() {
        let dir = tempfile::tempdir().unwrap();
        let (cert_pem, key_pem, cert_der, key_der) = generated_pem();
        let source = from_file(dir.path(), &cert_pem, &key_pem);

        let (certs, key) = source.resolve_der().unwrap();
        assert_eq!(certs, vec![cert_der.clone()]);
        assert_eq!(key, key_der);

        let certified = source.resolve().unwrap();
        assert_eq!(certified.cert.len(), 1);
        assert_eq!(certified.cert[0].as_ref(), cert_der.as_slice());
    }

    #[test]
    fn from_file_rejects_key_file_without_private_key() {
        let dir = tempfile::tempdir().unwrap();
        let (cert_pem, _, _, _) = generated_pem();
        // key file に cert しか無い (= 秘密鍵の section が無い)
        let source = from_file(dir.path(), &cert_pem, &cert_pem);

        let err = source.resolve_der().unwrap_err();
        assert!(
            format!("{err:#}").contains("no private key found"),
            "{err:#}"
        );
        let err = source.resolve().unwrap_err();
        assert!(
            format!("{err:#}").contains("no private key found"),
            "{err:#}"
        );
    }

    #[test]
    fn from_file_rejects_cert_file_without_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let (_, key_pem, _, _) = generated_pem();
        // cert file に秘密鍵しか無い
        let source = from_file(dir.path(), &key_pem, &key_pem);

        let err = source.resolve_der().unwrap_err();
        assert!(
            format!("{err:#}").contains("no certificates found"),
            "{err:#}"
        );
        let err = source.resolve().unwrap_err();
        assert!(
            format!("{err:#}").contains("no certificates found"),
            "{err:#}"
        );
    }

    #[test]
    fn from_file_rejects_broken_cert_pem() {
        let dir = tempfile::tempdir().unwrap();
        let (_, key_pem, _, _) = generated_pem();
        let broken = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
        let source = from_file(dir.path(), broken, &key_pem);

        let err = source.resolve_der().unwrap_err();
        assert!(
            format!("{err:#}").contains("failed to parse cert PEM"),
            "{err:#}"
        );
        let err = source.resolve().unwrap_err();
        assert!(
            format!("{err:#}").contains("failed to parse cert PEM"),
            "{err:#}"
        );
    }
}
