//! WebTransport ingress (= Phase 6a)。
//!
//! # なぜ WebTransport か
//!
//! v1.0 の Unison はブラウザファースト。 ブラウザは raw QUIC へ到達できず、
//! WebTransport (= QUIC + HTTP/3) のみがブラウザ ⇄ サーバー間の唯一の経路。
//! このモジュールは raw QUIC ([`super::quic::QuicServer`]) と **並立** する
//! WebTransport ingress を提供する。
//!
//! # アーキテクチャ上の位置
//!
//! `wtransport` の型 (`Connection` / `SendStream` / `RecvStream`) に
//! [`UnisonConn`] / [`UnisonSend`] / [`UnisonRecv`] を実装し、 raw QUIC と
//! **同一の** [`handle_connection`](super::quic) へ流し込む。 ハンドラー層
//! (`register_channel` / `UnisonStream`) は transport の種類を一切意識しない。
//!
//! ```text
//!   raw QUIC ingress ─┐
//!                     ├─► handle_connection ─► register_channel handlers
//!   WebTransport ─────┘     (Arc<dyn UnisonConn>)
//! ```
//!
//! # 証明書
//!
//! dev quickstart は自己署名証明書を使う。 ブラウザは未知の CA を拒否するが、
//! WebTransport は `serverCertificateHashes` による **SPKI ハッシュ pinning** を
//! サポートしており、 TS SDK 側 (`clients/typescript` の `trust.ts`) が
//! cert-hash pinning に対応している。 [`WebTransportServer::certificate_hash`]
//! が pin 用のハッシュ (= SHA-256, hex) を返す。

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{error, info};
use wtransport::{Endpoint, Identity, ServerConfig};

use super::conn::{BiStream, BoxUnisonRecv, BoxUnisonSend, UnisonConn, UnisonRecv, UnisonSend};
use super::context::ConnectionContext;
use super::dispatch::handle_connection;
use super::server::ProtocolServer;
use super::{NetworkError, cert::CertSource};

// ─────────────────────────────────────────
// UnisonConn 実装 — wtransport
// ─────────────────────────────────────────

/// `wtransport::SendStream` を [`UnisonSend`] として扱う。
///
/// `wtransport::SendStream` は `tokio::io::AsyncWrite` を実装済み。
impl UnisonSend for wtransport::SendStream {
    fn finish(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), NetworkError>> + Send + '_>> {
        Box::pin(async move {
            // wtransport の finish は async。 既に閉じられている等の失敗は
            // 正常終了扱いにする (= 冪等、 raw QUIC 側と挙動を揃える)。
            let _ = wtransport::SendStream::finish(self).await;
            Ok(())
        })
    }
}

/// `wtransport::RecvStream` を [`UnisonRecv`] として扱う。
impl UnisonRecv for wtransport::RecvStream {
    fn stop(&mut self) -> Result<(), NetworkError> {
        // `wtransport::RecvStream::stop` は `self` を消費するシグネチャのため
        // `&mut self` からは呼べない。 ストリームが drop された時点で QUIC が
        // STOP_SENDING / reset を送るため、 ここでは no-op で十分。
        Ok(())
    }
}

/// `wtransport::Connection` を transport-agnostic な [`UnisonConn`] として公開する。
impl UnisonConn for wtransport::Connection {
    fn accept_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>
    {
        Box::pin(async move {
            let (send, recv) = wtransport::Connection::accept_bi(self)
                .await
                .map_err(|e| NetworkError::Quic(format!("wt accept_bi failed: {}", e)))?;
            let send: BoxUnisonSend = Box::new(send);
            let recv: BoxUnisonRecv = Box::new(recv);
            Ok((send, recv))
        })
    }

    fn open_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>
    {
        Box::pin(async move {
            // wtransport の open_bi は 2 段階 await (= flow control → 初期化)。
            let (send, recv) = wtransport::Connection::open_bi(self)
                .await
                .map_err(|e| NetworkError::Quic(format!("wt open_bi (phase 1) failed: {}", e)))?
                .await
                .map_err(|e| NetworkError::Quic(format!("wt open_bi (phase 2) failed: {}", e)))?;
            let send: BoxUnisonSend = Box::new(send);
            let recv: BoxUnisonRecv = Box::new(recv);
            Ok((send, recv))
        })
    }

    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), NetworkError> {
        wtransport::Connection::send_datagram(self, data)
            .map_err(|e| NetworkError::Quic(format!("wt send_datagram failed: {}", e)))
    }

    fn recv_datagram(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bytes::Bytes, NetworkError>> + Send + '_>>
    {
        Box::pin(async move {
            let datagram = wtransport::Connection::receive_datagram(self)
                .await
                .map_err(|e| NetworkError::Quic(format!("wt recv_datagram failed: {}", e)))?;
            Ok(datagram.payload())
        })
    }

    fn remote_address(&self) -> SocketAddr {
        wtransport::Connection::remote_address(self)
    }

    fn close(&self, code: u32, reason: &[u8]) {
        wtransport::Connection::close(self, wtransport::VarInt::from_u32(code), reason);
    }
}

// ─────────────────────────────────────────
// WebTransport サーバー
// ─────────────────────────────────────────

/// WebTransport ingress サーバー。
///
/// raw QUIC の [`QuicServer`](super::quic::QuicServer) と並立し、 ブラウザ
/// クライアントの受け口になる。 受け付けた接続は raw QUIC と同一の
/// `handle_connection` へ流れる。
pub struct WebTransportServer {
    server: Arc<ProtocolServer>,
    cert_source: CertSource,
    endpoint: Option<Endpoint<wtransport::endpoint::endpoint_side::Server>>,
    /// leaf 証明書の SHA-256 ハッシュ (= BytesArray 形式)。
    /// ブラウザの `serverCertificateHashes` pinning 用、 `bind` 後に確定する。
    certificate_hash: Option<String>,
    /// leaf 証明書の SHA-256 ハッシュ (= 区切り無しの 64 文字 hex)。
    /// TS SDK の `trust.ts` (`certHash`) はこの plain hex 形式を期待する。
    certificate_hash_hex: Option<String>,
}

impl WebTransportServer {
    /// 新しい WebTransport サーバーを作る。
    ///
    /// `cert_source` は raw QUIC 側と同じ [`CertSource`] を共有でき、 TLS の
    /// 信頼モデルを 2 つの ingress で統一できる。
    pub fn new(server: Arc<ProtocolServer>, cert_source: CertSource) -> Self {
        Self {
            server,
            cert_source,
            endpoint: None,
            certificate_hash: None,
            certificate_hash_hex: None,
        }
    }

    /// dev quickstart: `localhost` 自己署名証明書で構成する。
    pub fn dev(server: Arc<ProtocolServer>) -> Self {
        Self::new(server, CertSource::dev_localhost())
    }

    /// 指定アドレスに bind する。
    ///
    /// [`CertSource`] を `wtransport::Identity` へ変換し、 HTTP/3 over QUIC の
    /// エンドポイントを開く。
    pub async fn bind(&mut self, addr: SocketAddr) -> Result<()> {
        let identity = cert_source_to_identity(&self.cert_source)?;

        // identity は ServerConfig へ move されるため、 先に cert hash を控える。
        let digest = identity.certificate_chain().as_slice()[0].hash();
        let cert_hash = digest.fmt(wtransport::tls::Sha256DigestFmt::BytesArray);
        // TS SDK 向けの区切り無し 64 文字 hex。 ブラウザの
        // `serverCertificateHashes` は leaf 証明書 DER 全体の SHA-256 を取る。
        let cert_hash_hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();

        let config = ServerConfig::builder()
            .with_bind_address(addr)
            .with_identity(identity)
            .keep_alive_interval(Some(std::time::Duration::from_secs(10)))
            .max_idle_timeout(Some(std::time::Duration::from_secs(60)))
            .context("wtransport max_idle_timeout 設定に失敗")?
            .build();

        let endpoint = Endpoint::server(config).context("WebTransport endpoint の生成に失敗")?;

        info!(
            "WebTransport server bound to {} (cert hash: {})",
            addr, cert_hash
        );
        self.endpoint = Some(endpoint);
        self.certificate_hash = Some(cert_hash);
        self.certificate_hash_hex = Some(cert_hash_hex);
        Ok(())
    }

    /// bind 済みのローカルアドレスを取得する。
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.endpoint.as_ref().and_then(|ep| ep.local_addr().ok())
    }

    /// leaf 証明書の SHA-256 ハッシュ (= BytesArray 形式) を取得する。
    ///
    /// ブラウザ側 (`WebTransport` の `serverCertificateHashes`) がこの値で
    /// 自己署名証明書を pin する。 `bind` 前は `None`。
    pub fn certificate_hash(&self) -> Option<&str> {
        self.certificate_hash.as_deref()
    }

    /// leaf 証明書の SHA-256 ハッシュを区切り無しの 64 文字 hex で取得する。
    ///
    /// [`Self::certificate_hash`] が返す `BytesArray` 形式に対し、 こちらは
    /// TS SDK の `trust.ts` (`{ certHash }`) がそのまま受け取れる plain hex。
    /// `bind` 前は `None`。
    pub fn certificate_hash_hex(&self) -> Option<&str> {
        self.certificate_hash_hex.as_deref()
    }

    /// 接続の待ち受けを開始する (= 終了までブロック)。
    pub async fn start(&self) -> Result<()> {
        let endpoint = self
            .endpoint
            .as_ref()
            .context("WebTransport server not bound")?;

        info!("WebTransport server listening for sessions");

        loop {
            let incoming = endpoint.accept().await;
            let server = Arc::clone(&self.server);
            tokio::spawn(async move {
                if let Err(e) = accept_session(incoming, server).await {
                    error!("WebTransport session error: {}", e);
                }
            });
        }
    }

    /// shutdown シグナル対応版の [`Self::start`]。
    pub async fn start_with_shutdown(
        &self,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let endpoint = self
            .endpoint
            .as_ref()
            .context("WebTransport server not bound")?;

        info!("WebTransport server listening for sessions (with shutdown support)");

        loop {
            tokio::select! {
                incoming = endpoint.accept() => {
                    let server = Arc::clone(&self.server);
                    tokio::spawn(async move {
                        if let Err(e) = accept_session(incoming, server).await {
                            error!("WebTransport session error: {}", e);
                        }
                    });
                }
                _ = &mut shutdown_rx => {
                    info!("Shutdown signal received, stopping WebTransport server");
                    endpoint.close(wtransport::VarInt::from_u32(0), b"server shutdown");
                    break;
                }
            }
        }

        Ok(())
    }
}

/// 1 つの incoming WebTransport セッションを受理し、 `handle_connection` へ渡す。
///
/// HTTP/3 の WebTransport ハンドシェイク (= incoming → session request →
/// session accept) を完了させた上で、 raw QUIC と同一の接続ハンドラーへ
/// 接続を流す。
async fn accept_session(
    incoming: wtransport::endpoint::IncomingSession,
    server: Arc<ProtocolServer>,
) -> Result<()> {
    // QUIC ハンドシェイク完了を待ち、 WebTransport セッション要求を取得。
    let session_request = incoming
        .await
        .context("WebTransport セッション要求の取得に失敗")?;

    let remote = session_request.remote_address();
    info!(
        "New WebTransport session from {} (path: {})",
        remote,
        session_request.path()
    );

    // セッションを accept して接続を確立。
    let connection = session_request
        .accept()
        .await
        .context("WebTransport セッションの accept に失敗")?;

    // raw QUIC と同一の handle_connection へ。 transport の違いはここで吸収される。
    let conn: Arc<dyn UnisonConn> = Arc::new(connection);
    let ctx = Arc::new(ConnectionContext::new());
    handle_connection(conn, server, ctx).await
}

/// [`CertSource`] を `wtransport::Identity` へ変換する。
///
/// raw QUIC 側と TLS マテリアルを共有するためのブリッジ。 `cert.rs` の
/// [`CertSource`] が解決した DER (= cert chain + private key) を `wtransport`
/// の [`Identity`] へ詰め替える。
fn cert_source_to_identity(source: &CertSource) -> Result<Identity> {
    use wtransport::tls::{Certificate, CertificateChain, PrivateKey};

    let (cert_chain_der, key_der) = source.resolve_der()?;

    let certs = cert_chain_der
        .into_iter()
        .map(Certificate::from_der)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("wtransport Certificate の DER parse に失敗")?;
    let chain = CertificateChain::new(certs);
    let key = PrivateKey::from_der_pkcs8(key_der);

    Ok(Identity::new(chain, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dev 自己署名 [`CertSource`] が `wtransport::Identity` へ変換でき、
    /// leaf 証明書から SHA-256 ハッシュを取り出せること。
    #[test]
    fn cert_source_converts_to_identity_with_hash() {
        let source = CertSource::dev_localhost();
        let identity = cert_source_to_identity(&source).expect("dev cert は変換できるべき");
        let hash = identity.certificate_chain().as_slice()[0]
            .hash()
            .fmt(wtransport::tls::Sha256DigestFmt::BytesArray);
        // SHA-256 BytesArray 形式 = 32 byte の hex (= 0x.., 区切り) で非空。
        assert!(!hash.is_empty(), "cert hash は非空であるべき");
    }

    /// `CertSource::Provided` は秘密鍵 DER を持たないため `resolve_der` がエラーを返す。
    #[test]
    fn provided_cert_source_rejects_der_resolution() {
        let source = CertSource::dev_localhost();
        let certified = source.resolve().expect("dev cert resolve は成功するべき");
        let provided = CertSource::Provided {
            certified_key: certified,
        };
        assert!(
            provided.resolve_der().is_err(),
            "Provided は resolve_der でエラーを返すべき"
        );
    }

    /// WebTransport サーバーが ephemeral port に bind でき、 bind 後に cert hash が
    /// 確定し、 local_addr が取得できること (= 実 ingress の起動確認)。
    #[tokio::test]
    async fn webtransport_server_binds_and_exposes_cert_hash() {
        let protocol = Arc::new(ProtocolServer::new());
        let mut wt = WebTransportServer::dev(protocol);

        // bind 前は cert hash 未確定。
        assert!(wt.certificate_hash().is_none());

        // ephemeral port (= 0) に bind。
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        wt.bind(addr)
            .await
            .expect("WebTransport bind は成功するべき");

        // bind 後は local_addr と cert hash が取れる。
        let local = wt.local_addr().expect("local_addr が取れるべき");
        assert_ne!(local.port(), 0, "ephemeral port が割り当てられるべき");
        assert!(
            wt.certificate_hash().is_some_and(|h| !h.is_empty()),
            "bind 後は cert hash が確定するべき"
        );

        // plain hex 形式 (= TS SDK 向け) は区切り無しの 64 文字 hex。
        let hex = wt
            .certificate_hash_hex()
            .expect("bind 後は plain hex cert hash も確定するべき");
        assert_eq!(hex.len(), 64, "SHA-256 は 64 文字 hex であるべき");
        assert!(
            hex.bytes().all(|b| b.is_ascii_hexdigit()),
            "plain hex は hex 文字のみであるべき"
        );
    }
}
