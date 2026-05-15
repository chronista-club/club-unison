use anyhow::{Context, Result};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig as RustlsClientConfig, ServerConfig as RustlsServerConfig};
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use super::{
    NetworkError, ProtocolFrame, ProtocolMessage, context::ConnectionContext,
    server::ProtocolServer,
};

/// Default certificate file paths for assets/certs directory
pub const DEFAULT_CERT_PATH: &str = "assets/certs/cert.pem";
pub const DEFAULT_KEY_PATH: &str = "assets/certs/private_key.der";

/// Maximum message size for QUIC streams (8MB)
const MAX_MESSAGE_SIZE: usize = 8 * 1024 * 1024;

/// Default port for QUIC connections
const DEFAULT_PORT: u16 = 8080;

/// アドレス文字列を SocketAddr に解決する共通関数。
///
/// IPv6 / IPv4 リテラル + DNS hostname を受け付け、必要に応じて DNS 解決する。
///
/// 対応形式:
/// - `[::1]:8080` — IPv6 リテラル + port
/// - `::1` — IPv6 のみ (DEFAULT_PORT 付与)
/// - `1.2.3.4:8080` — IPv4 リテラル + port
/// - `8080` — port のみ (IPv6 ループバック fallback)
/// - `localhost:8080` / `localhost` — DNS 解決
/// - `host.example.com:8080` / `host.example.com` — DNS 解決
/// - `https://host:port` / `http://host:port` / `quic://host:port` — scheme prefix を strip
///
/// DNS 解決時は最初の resolved address を返す (IPv4/IPv6 どちらでも、リゾルバの順)。
async fn resolve_socket_addr(addr: &str) -> Result<SocketAddr> {
    // URL scheme 剥がし
    let addr = strip_scheme(addr);

    // 1. IPv4/IPv6 リテラル + port を直接 parse
    if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
        return Ok(socket_addr);
    }

    // 2. port のみ ("8080") → IPv6 ループバック (後方互換)
    if let Ok(port) = addr.parse::<u16>() {
        return Ok(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)));
    }

    // 3. IPv6 リテラル、port なし ("::1")
    if addr.contains(':') && !addr.contains('[') && !addr.contains('.') {
        let with_port = format!("[{}]:{}", addr, DEFAULT_PORT);
        if let Ok(sa) = with_port.parse::<SocketAddr>() {
            return Ok(sa);
        }
    }

    // 4. [IPv6]:port (bracket notation で port パース失敗ケース対応)
    if addr.starts_with('[')
        && let Some(end) = addr.find(']')
    {
        let ipv6_str = &addr[1..end];
        let ipv6 = ipv6_str
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| anyhow::anyhow!("無効なIPv6アドレス: {}", ipv6_str))?;
        let port = if addr.len() > end + 1 && &addr[end + 1..end + 2] == ":" {
            let port_str = &addr[end + 2..];
            if port_str.is_empty() {
                DEFAULT_PORT
            } else {
                port_str
                    .parse::<u16>()
                    .map_err(|_| anyhow::anyhow!("無効なポート番号: {}", port_str))?
            }
        } else {
            DEFAULT_PORT
        };
        return Ok(SocketAddr::from((ipv6, port)));
    }

    // 5. DNS hostname (host or host:port)
    let lookup_target = if has_port(addr) {
        addr.to_string()
    } else {
        format!("{}:{}", addr, DEFAULT_PORT)
    };
    let mut iter = tokio::net::lookup_host(&lookup_target)
        .await
        .with_context(|| format!("DNS lookup 失敗: {}", lookup_target))?;
    iter.next()
        .with_context(|| format!("アドレスを解決できませんでした: {}", lookup_target))
}

/// `https://` / `http://` / `quic://` 前置詞を取り除く
fn strip_scheme(addr: &str) -> &str {
    addr.strip_prefix("https://")
        .or_else(|| addr.strip_prefix("http://"))
        .or_else(|| addr.strip_prefix("quic://"))
        .unwrap_or(addr)
}

/// アドレスが `host:port` 形式 (末尾に port が付いている) か判定。
/// IPv6 リテラルは bracket notation 限定で判定する (生 `::1` は port 無し扱い)。
fn has_port(addr: &str) -> bool {
    if addr.starts_with('[') {
        return addr.contains("]:");
    }
    // 単純な hostname or IPv4 — 末尾の `:NNN` を port として認識
    if let Some(colon) = addr.rfind(':') {
        // host:port の host 側に ':' が無い (= IPv6 ではない) ことを担保
        if !addr[..colon].contains(':') {
            return addr[colon + 1..].parse::<u16>().is_ok();
        }
    }
    false
}

/// Length-prefixed フレームの読み取り（4バイトBE長 + データ）
/// ストリームを消費せずに1フレームだけ読む
pub async fn read_frame(recv: &mut RecvStream) -> Result<bytes::Bytes> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .context("Failed to read frame length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE {
        return Err(anyhow::anyhow!("Frame too large: {} bytes", len));
    }
    let mut data = vec![0u8; len];
    recv.read_exact(&mut data)
        .await
        .context("Failed to read frame data")?;
    Ok(bytes::Bytes::from(data))
}

/// Length-prefixed フレームの書き込み
pub async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    let len = (data.len() as u32).to_be_bytes();
    send.write_all(&len)
        .await
        .context("Failed to write frame length")?;
    send.write_all(data)
        .await
        .context("Failed to write frame data")?;
    Ok(())
}

/// フレームタイプタグ
pub const FRAME_TYPE_PROTOCOL: u8 = 0x00;
pub const FRAME_TYPE_RAW: u8 = 0x01;

/// Typed フレーム — type tag 付きの読み書き
/// フォーマット: [4 bytes: length][1 byte: type tag][payload]
/// length は type tag + payload の合計バイト数
///
/// Typed フレームの読み取り — type tag とペイロードを返す
pub async fn read_typed_frame(recv: &mut RecvStream) -> Result<(u8, bytes::Bytes)> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .context("Failed to read frame length")?;
    let total_len = u32::from_be_bytes(len_buf) as usize;
    if total_len == 0 {
        return Err(anyhow::anyhow!("Empty frame"));
    }
    if total_len > MAX_MESSAGE_SIZE {
        return Err(anyhow::anyhow!("Frame too large: {} bytes", total_len));
    }

    // type tag を読む
    let mut type_buf = [0u8; 1];
    recv.read_exact(&mut type_buf)
        .await
        .context("Failed to read frame type tag")?;
    let frame_type = type_buf[0];

    // payload を読む
    let payload_len = total_len - 1;
    let mut data = vec![0u8; payload_len];
    recv.read_exact(&mut data)
        .await
        .context("Failed to read frame payload")?;
    Ok((frame_type, bytes::Bytes::from(data)))
}

/// Typed フレームの書き込み
pub async fn write_typed_frame(send: &mut SendStream, frame_type: u8, data: &[u8]) -> Result<()> {
    let total_len = (1 + data.len()) as u32;
    send.write_all(&total_len.to_be_bytes())
        .await
        .context("Failed to write frame length")?;
    send.write_all(&[frame_type])
        .await
        .context("Failed to write frame type tag")?;
    send.write_all(data)
        .await
        .context("Failed to write frame payload")?;
    Ok(())
}

/// QUIC client implementation
pub struct QuicClient {
    endpoint: Mutex<Option<Endpoint>>,
    connection: Arc<RwLock<Option<Connection>>>,
    rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<ProtocolMessage>>>>,
    tx: mpsc::UnboundedSender<ProtocolMessage>,
    /// Identity handshake 専用の oneshot チャネル（受信側）
    identity_rx: Arc<Mutex<Option<oneshot::Receiver<ProtocolMessage>>>>,
    /// Identity handshake 専用の oneshot チャネル（送信側、accept_bi_loop に渡す）
    identity_tx: Arc<Mutex<Option<oneshot::Sender<ProtocolMessage>>>>,
    /// レスポンス受信タスクのハンドルを管理
    response_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// Trust anchors used when verifying the server's certificate during connect.
    ///
    /// v0.8.0: explicit per-instance trust selection. Defaults to
    /// `TrustAnchors::SkipVerification` for backward compatibility with
    /// `QuicClient::new()` callers (will be tightened in v0.9.0).
    trust_anchors: super::trust::TrustAnchors,
}

/// Builder for [`QuicClient`] (v0.8.0+).
///
/// Use [`QuicClient::builder`] to construct.
pub struct QuicClientBuilder {
    trust_anchors: Option<super::trust::TrustAnchors>,
}

impl QuicClientBuilder {
    /// Set the trust anchor source used to verify server certs on `connect`.
    pub fn trust_anchors(mut self, trust: super::trust::TrustAnchors) -> Self {
        self.trust_anchors = Some(trust);
        self
    }

    /// Build the [`QuicClient`]. If `trust_anchors` is not set, defaults to
    /// [`super::trust::TrustAnchors::SkipVerification`] for backward
    /// compatibility — a `tracing::warn!` is emitted at connect time.
    pub fn build(self) -> Result<QuicClient> {
        let trust_anchors = self
            .trust_anchors
            .unwrap_or(super::trust::TrustAnchors::SkipVerification);
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(QuicClient {
            endpoint: Mutex::new(None),
            connection: Arc::new(RwLock::new(None)),
            rx: Arc::new(RwLock::new(Some(rx))),
            tx,
            identity_rx: Arc::new(Mutex::new(None)),
            identity_tx: Arc::new(Mutex::new(None)),
            response_tasks: Arc::new(Mutex::new(Vec::new())),
            trust_anchors,
        })
    }
}

impl QuicClient {
    /// Builder entry point (v0.8.0+) — preferred over [`Self::new`].
    pub fn builder() -> QuicClientBuilder {
        QuicClientBuilder {
            trust_anchors: None,
        }
    }

    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Self {
            endpoint: Mutex::new(None),
            connection: Arc::new(RwLock::new(None)),
            rx: Arc::new(RwLock::new(Some(rx))),
            tx,
            identity_rx: Arc::new(Mutex::new(None)),
            identity_tx: Arc::new(Mutex::new(None)),
            response_tasks: Arc::new(Mutex::new(Vec::new())),
            trust_anchors: super::trust::TrustAnchors::SkipVerification,
        })
    }

    /// Configure client with a given trust anchor source.
    ///
    /// v0.7.0+: operator must explicitly choose how server certs are verified.
    /// See [`crate::network::trust::TrustAnchors`] for variants.
    pub async fn configure_client_with(trust: super::trust::TrustAnchors) -> Result<ClientConfig> {
        let rustls_client_config = trust.build_client_config()?;
        // ClientConfig is Arc<rustls::ClientConfig> — extract and rewrap for quinn
        let client_crypto_config: RustlsClientConfig = (*rustls_client_config).clone();
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto_config)?;
        let mut client_config = ClientConfig::new(Arc::new(crypto));

        let mut transport_config = quinn::TransportConfig::default();
        transport_config
            .max_idle_timeout(Some(std::time::Duration::from_secs(60).try_into().unwrap()));
        transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
        transport_config.max_concurrent_uni_streams(0u32.into());
        transport_config.max_concurrent_bidi_streams(1000u32.into());
        transport_config.initial_rtt(std::time::Duration::from_millis(100));
        client_config.transport_config(Arc::new(transport_config));

        Ok(client_config)
    }

    // 双方向ストリームを使うため、start_receive_loopは不要になりました

    /// QUIC接続への参照を取得（チャネル用ストリーム開設に使用）
    pub fn connection(&self) -> &Arc<RwLock<Option<Connection>>> {
        &self.connection
    }
}

impl QuicClient {
    /// サーバーアドレスを解析 (IPv4 / IPv6 / DNS hostname 対応)
    async fn parse_server_address(addr: &str) -> Result<SocketAddr> {
        resolve_socket_addr(addr).await
    }

    pub async fn receive(&self) -> Result<ProtocolMessage> {
        let mut rx_guard = self.rx.write().await;
        if let Some(rx) = rx_guard.as_mut() {
            rx.recv()
                .await
                .context("Failed to receive message from channel")
        } else {
            Err(anyhow::anyhow!("Receiver not available"))
        }
    }

    /// Identity 専用チャネルから identity メッセージを受信する（タイムアウト付き）
    pub async fn receive_identity(
        &self,
        timeout_duration: std::time::Duration,
    ) -> Result<ProtocolMessage> {
        let rx = self
            .identity_rx
            .lock()
            .await
            .take()
            .context("Identity receiver not available (already consumed or not connected)")?;

        tokio::time::timeout(timeout_duration, rx)
            .await
            .map_err(|_| anyhow::anyhow!("Identity handshake timed out"))?
            .map_err(|_| anyhow::anyhow!("Identity sender dropped without sending"))
    }

    pub async fn connect(&self, url: &str) -> Result<()> {
        // URL を解決 (IPv4 / IPv6 / DNS hostname)
        let addr = Self::parse_server_address(url).await?;

        // v0.8.0+: builder で設定された trust_anchors を使う (default = SkipVerification、
        // builder 経由で TrustAnchors::System 等に明示変更可能)
        let client_config = Self::configure_client_with(self.trust_anchors.clone()).await?;

        // bind addr は target family に揃える (IPv4 target には 0.0.0.0、IPv6 target には [::])
        let bind_addr: SocketAddr = match addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };

        let mut endpoint = Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_config);

        let connection = endpoint
            .connect(addr, "localhost")?
            .await
            .context("Failed to establish QUIC connection")?;

        info!("Connected to QUIC server at {}", addr);

        // Endpoint を保存（drop されると UDP ソケットが閉じて接続が切れる）
        *self.endpoint.lock().await = Some(endpoint);

        // accept_bi ループ用に connection をクローン
        let connection_for_loop = connection.clone();
        *self.connection.write().await = Some(connection);

        // Identity 専用の oneshot チャネルを作成
        let (id_tx, id_rx) = oneshot::channel();
        *self.identity_tx.lock().await = Some(id_tx);
        *self.identity_rx.lock().await = Some(id_rx);

        // サーバー発信ストリームを受け付けるバックグラウンドタスクを起動
        let tx = self.tx.clone();
        let identity_tx = self.identity_tx.clone();
        let task = tokio::spawn(async move {
            client_accept_bi_loop(connection_for_loop, tx, identity_tx).await;
        });
        self.response_tasks.lock().await.push(task);

        Ok(())
    }

    pub async fn disconnect(&self) -> Result<()> {
        // すべてのレスポンス受信タスクをキャンセル
        let mut tasks = self.response_tasks.lock().await;
        for task in tasks.drain(..) {
            task.abort();
        }

        // 接続をクローズ
        let mut connection_guard = self.connection.write().await;
        if let Some(connection) = connection_guard.take() {
            connection.close(quinn::VarInt::from_u32(0), b"client disconnect");
        }

        // Endpoint をクリーンアップ
        self.endpoint.lock().await.take();

        Ok(())
    }

    pub async fn is_connected(&self) -> bool {
        let connection_guard = self.connection.read().await;
        if let Some(connection) = connection_guard.as_ref() {
            connection.close_reason().is_none()
        } else {
            false
        }
    }
}

/// QUICサーバー実装
pub struct QuicServer {
    server: Arc<ProtocolServer>,
    endpoint: Option<Endpoint>,
    /// Cert source used by `bind` to configure the TLS server.
    ///
    /// v0.8.0+: explicit per-instance cert selection via [`QuicServer::builder`].
    /// Defaults to [`super::cert::CertSource::dev_localhost`] for backward
    /// compatibility with `QuicServer::new()`.
    cert_source: super::cert::CertSource,
}

/// Builder for [`QuicServer`] (v0.8.0+).
///
/// Use [`QuicServer::builder`] to construct.
pub struct QuicServerBuilder {
    server: Arc<ProtocolServer>,
    cert_source: Option<super::cert::CertSource>,
}

impl QuicServerBuilder {
    /// Set the cert source used to configure the TLS server at bind time.
    pub fn cert_source(mut self, cert: super::cert::CertSource) -> Self {
        self.cert_source = Some(cert);
        self
    }

    /// Build the [`QuicServer`]. If `cert_source` is not set, defaults to
    /// [`super::cert::CertSource::dev_localhost`] (DEV ONLY).
    pub fn build(self) -> QuicServer {
        QuicServer {
            server: self.server,
            endpoint: None,
            cert_source: self
                .cert_source
                .unwrap_or_else(super::cert::CertSource::dev_localhost),
        }
    }
}

impl QuicServer {
    /// Builder entry point (v0.8.0+) — preferred over [`Self::new`].
    pub fn builder(server: Arc<ProtocolServer>) -> QuicServerBuilder {
        QuicServerBuilder {
            server,
            cert_source: None,
        }
    }

    pub fn new(server: Arc<ProtocolServer>) -> Self {
        Self {
            server,
            endpoint: None,
            cert_source: super::cert::CertSource::dev_localhost(),
        }
    }

    /// QUIC/TLS 1.3用の自己署名証明書を生成（本番環境使用に最適化）
    pub fn generate_self_signed_cert()
    -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let subject_alt_names = vec![
            "localhost".to_string(),
            "*.unison.svc.cluster.local".to_string(),
            "dev.chronista.club".to_string(),
        ];

        let cert_key = rcgen::generate_simple_self_signed(subject_alt_names)?;
        let cert_der_bytes = cert_key.cert.der().to_vec();
        let private_key_der_bytes = cert_key.signing_key.serialize_der();

        Ok((
            vec![CertificateDer::from(cert_der_bytes)],
            PrivateKeyDer::try_from(private_key_der_bytes).unwrap(),
        ))
    }

    /// 外部ファイルから証明書を読み込み（本番環境デプロイ用）
    pub fn load_cert_from_files(
        cert_path: &str,
        key_path: &str,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert_pem = std::fs::read_to_string(cert_path)?;
        let key_der = std::fs::read(key_path)?;

        let cert_chain = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to parse certificate")?;
        let certs = cert_chain;

        // Convert to owned data for static lifetime
        let key_der_owned = key_der.clone();
        let private_key = PrivateKeyDer::try_from(key_der_owned.as_ref())
            .map_err(|e| anyhow::anyhow!("Failed to parse private key: {}", e))?;

        Ok((certs, private_key.clone_key()))
    }

    /// Configure server with TLS, given a [`CertSource`].
    ///
    /// v0.7.0+: operator must explicitly choose how to obtain the certificate.
    /// See [`crate::network::cert::CertSource`] for variants.
    pub async fn configure_server_with(
        cert_source: super::cert::CertSource,
    ) -> Result<ServerConfig> {
        let certified_key = cert_source.resolve()?;

        // CertifiedKey holds both cert chain and signing key in a single Arc,
        // avoiding any clone_key() of the private key (zeroize-friendlier).
        let rustls_server_config = RustlsServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleCertResolver(certified_key)));

        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(rustls_server_config)?;
        let mut server_config = ServerConfig::with_crypto(Arc::new(crypto));

        let mut transport_config = quinn::TransportConfig::default();
        transport_config
            .max_idle_timeout(Some(std::time::Duration::from_secs(60).try_into().unwrap()));
        transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
        transport_config.max_concurrent_uni_streams(0u32.into());
        transport_config.max_concurrent_bidi_streams(1000u32.into());
        transport_config.initial_rtt(std::time::Duration::from_millis(100));
        server_config.transport_config(Arc::new(transport_config));

        Ok(server_config)
    }

    pub async fn bind(&mut self, addr: &str) -> Result<()> {
        // IPv4 / IPv6 / DNS hostname のいずれにも対応
        let socket_addr = Self::parse_socket_addr(addr).await?;

        // v0.8.0+: builder で設定された cert_source を使う (default = dev_localhost、
        // builder 経由で Provided / FromFile / internal_mesh に明示変更可能)
        let server_config = Self::configure_server_with(self.cert_source.clone()).await?;
        let endpoint = Endpoint::server(server_config, socket_addr)?;

        info!("QUIC server bound to {}", socket_addr);
        self.endpoint = Some(endpoint);
        Ok(())
    }

    /// ソケットアドレスを解析 (IPv4 / IPv6 / DNS hostname 対応)
    async fn parse_socket_addr(addr: &str) -> Result<SocketAddr> {
        resolve_socket_addr(addr).await
    }

    /// バインド済みのローカルアドレスを取得
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.endpoint.as_ref().and_then(|ep| ep.local_addr().ok())
    }

    pub async fn start(&self) -> Result<()> {
        let endpoint = self
            .endpoint
            .as_ref()
            .context("Server not bound to an address")?;

        info!("QUIC server listening for connections");

        while let Some(connecting) = endpoint.accept().await {
            let connection = connecting.await?;
            let remote_addr = connection.remote_address();
            info!("New QUIC connection from: {}", remote_addr);

            let server = Arc::clone(&self.server);
            let ctx = Arc::new(ConnectionContext::new());
            tokio::spawn(async move {
                if let Err(e) = handle_connection(connection, server, ctx).await {
                    error!("Connection error: {}", e);
                }
            });
        }

        Ok(())
    }

    /// shutdown シグナルを受け付けるバージョンの start
    pub async fn start_with_shutdown(
        &self,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let endpoint = self
            .endpoint
            .as_ref()
            .context("Server not bound to an address")?;

        info!("QUIC server listening for connections (with shutdown support)");

        loop {
            tokio::select! {
                connecting = endpoint.accept() => {
                    match connecting {
                        Some(connecting) => {
                            let connection = connecting.await?;
                            let remote_addr = connection.remote_address();
                            info!("New QUIC connection from: {}", remote_addr);

                            let server = Arc::clone(&self.server);
                            let ctx = Arc::new(ConnectionContext::new());
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(connection, server, ctx).await {
                                    error!("Connection error: {}", e);
                                }
                            });
                        }
                        None => {
                            info!("QUIC endpoint closed");
                            break;
                        }
                    }
                }
                _ = &mut shutdown_rx => {
                    info!("Shutdown signal received, stopping server");
                    endpoint.close(quinn::VarInt::from_u32(0), b"server shutdown");
                    break;
                }
            }
        }

        Ok(())
    }
}

/// クライアント側: サーバー発信の双方向ストリームを受け付けるループ
///
/// サーバーが `connection.open_bi()` で開いたストリーム（Identity 送信等）を
/// `accept_bi()` で受信し、ProtocolMessage に変換する。
/// `__identity` メッセージは専用の oneshot チャネルに送り、それ以外は既存の mpsc に送る。
async fn client_accept_bi_loop(
    connection: Connection,
    tx: mpsc::UnboundedSender<ProtocolMessage>,
    identity_tx: Arc<Mutex<Option<oneshot::Sender<ProtocolMessage>>>>,
) {
    loop {
        match connection.accept_bi().await {
            Ok((_send_stream, mut recv_stream)) => {
                let tx = tx.clone();
                let identity_tx = identity_tx.clone();
                tokio::spawn(async move {
                    match read_typed_frame(&mut recv_stream).await {
                        Ok((FRAME_TYPE_PROTOCOL, frame_bytes)) => {
                            if let Ok(frame) = ProtocolFrame::from_bytes(&frame_bytes)
                                && let Ok(message) = ProtocolMessage::from_frame(&frame)
                            {
                                if message.method == "__identity" {
                                    // Identity メッセージは専用 oneshot チャネルに送信
                                    if let Some(id_tx) = identity_tx.lock().await.take() {
                                        let _ = id_tx.send(message);
                                    } else {
                                        warn!(
                                            "Identity oneshot already consumed, dropping identity message"
                                        );
                                    }
                                } else {
                                    // それ以外は既存の mpsc チャネルに送信
                                    let _ = tx.send(message);
                                }
                            }
                        }
                        Ok((frame_type, _)) => {
                            warn!(
                                "Unexpected frame type in server-initiated stream: 0x{:02x}",
                                frame_type
                            );
                        }
                        Err(e) => {
                            warn!("Failed to read server-initiated stream: {}", e);
                        }
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                info!("Connection closed by server");
                break;
            }
            Err(e) => {
                warn!("Failed to accept server-initiated stream: {}", e);
                break;
            }
        }
    }
}

async fn handle_connection(
    connection: Connection,
    server: Arc<ProtocolServer>,
    ctx: Arc<ConnectionContext>,
) -> Result<()> {
    let remote_addr = connection.remote_address();

    // Identity Handshake: 接続直後にServerIdentityを送信
    let identity = server.build_identity().await;
    ctx.set_identity(identity.clone()).await;

    let identity_msg = identity.to_protocol_message();
    match identity_msg.into_frame() {
        Ok(frame) => {
            let frame_bytes = frame.to_bytes();
            match connection.open_bi().await {
                Ok((mut send_stream, _recv_stream)) => {
                    if let Err(e) =
                        write_typed_frame(&mut send_stream, FRAME_TYPE_PROTOCOL, &frame_bytes).await
                    {
                        warn!("Failed to send identity: {}", e);
                    } else {
                        let _ = send_stream.finish();
                        info!("Identity sent to client");
                    }
                }
                Err(e) => {
                    warn!("Failed to open identity stream: {}", e);
                }
            }
        }
        Err(e) => {
            warn!("Failed to serialize identity frame: {}", e);
        }
    }

    // 接続イベントを送信
    server.emit_connection_event(super::server::ConnectionEvent::Connected {
        remote_addr,
        context: Arc::clone(&ctx),
    });

    loop {
        let connection_clone = connection.clone();
        match connection.accept_bi().await {
            Ok((send_stream, mut recv_stream)) => {
                let server = Arc::clone(&server);
                let connection = connection_clone;
                let ctx = Arc::clone(&ctx);

                tokio::spawn(async move {
                    // typed frame で読み取り（type tag 付き）
                    let request_result = match read_typed_frame(&mut recv_stream).await {
                        Ok((FRAME_TYPE_PROTOCOL, frame_bytes)) => {
                            ProtocolFrame::from_bytes(&frame_bytes)
                                .and_then(|frame| ProtocolMessage::from_frame(&frame))
                        }
                        Ok((frame_type, _)) => {
                            warn!("Unexpected frame type in handshake: 0x{:02x}", frame_type);
                            return;
                        }
                        Err(e) => {
                            error!("Failed to read handshake frame: {}", e);
                            return;
                        }
                    };

                    match request_result {
                        Ok(request) => {
                            // チャネルルーティング: __channel: プレフィックスをチェック
                            if let Some(channel_name) = request.method.strip_prefix("__channel:") {
                                let channel_name = channel_name.to_string();
                                if let Some(handler) =
                                    server.get_channel_handler(&channel_name).await
                                {
                                    // channel lifecycle の "open" 側ログ。
                                    // close 側 (= 下記の debug!) と対になり、 1 接続中の
                                    // channel 開閉 trace が debug level で揃う。
                                    // info level にしない理由: 1 接続で channel が頻繁に
                                    // open/close される設計 (= 1 request/response = 1 channel)
                                    // なので info noise になりがち。
                                    debug!("Channel '{}' opened", channel_name);

                                    // チャネル用のUnisonStreamを作成（ストリームは生きたまま）
                                    let stream = UnisonStream::from_streams(
                                        request.id,
                                        request.method.clone(),
                                        Arc::new(connection),
                                        send_stream,
                                        recv_stream,
                                    );
                                    if let Err(e) = handler(ctx, stream).await {
                                        // sender 側が request/response 完了後に正常 close した
                                        // end-of-stream は real error ではないので debug level に
                                        // degrade。 これにより毎 channel session の終端で発生する
                                        // ERROR log noise (= journal で大半を占める) を抑制。
                                        if e.is_normal_close() {
                                            debug!(
                                                "Channel '{}' closed normally (end of stream)",
                                                channel_name
                                            );
                                        } else {
                                            error!(
                                                "Channel handler error for '{}': {}",
                                                channel_name, e
                                            );
                                        }
                                    }
                                } else {
                                    warn!("No channel handler for: {}", channel_name);
                                }
                                return;
                            }

                            // 非チャネルメッセージはサポート外
                            warn!(
                                "Non-channel message received (method: {}). Use channels instead.",
                                request.method
                            );
                        }
                        Err(e) => {
                            warn!("Failed to parse message: {}", e);
                        }
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                info!("Client disconnected");
                server.emit_connection_event(super::server::ConnectionEvent::Disconnected {
                    remote_addr,
                });
                break;
            }
            Err(e) => {
                error!("Failed to accept stream: {}", e);
                server.emit_connection_event(super::server::ConnectionEvent::Disconnected {
                    remote_addr,
                });
                break;
            }
        }
    }

    Ok(())
}

/// Server-side cert resolver that always returns the same [`rustls::sign::CertifiedKey`].
///
/// Holds the key behind a single `Arc` so the private key material exists in
/// memory exactly once for the lifetime of the server.
#[derive(Debug)]
struct SingleCertResolver(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for SingleCertResolver {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// Unison Stream - QUIC双方向ストリーム実装
pub struct UnisonStream {
    stream_id: u64,
    method: String,
    #[allow(dead_code)]
    connection: Arc<Connection>,
    send_stream: Arc<Mutex<Option<SendStream>>>,
    recv_stream: Arc<Mutex<Option<RecvStream>>>,
    is_active: Arc<AtomicBool>,
}

impl UnisonStream {
    pub async fn new(
        method: String,
        connection: Arc<Connection>,
        stream_id: Option<u64>,
    ) -> Result<Self> {
        static STREAM_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

        let id = stream_id.unwrap_or_else(|| STREAM_ID_COUNTER.fetch_add(1, Ordering::SeqCst));

        // Open bidirectional stream
        let (send_stream, recv_stream) = connection
            .open_bi()
            .await
            .context("Failed to open bidirectional stream")?;

        Ok(Self {
            stream_id: id,
            method,
            connection,
            send_stream: Arc::new(Mutex::new(Some(send_stream))),
            recv_stream: Arc::new(Mutex::new(Some(recv_stream))),
            is_active: Arc::new(AtomicBool::new(true)),
        })
    }

    /// 既存のストリームから作成（サーバー側）
    pub fn from_streams(
        stream_id: u64,
        method: String,
        connection: Arc<Connection>,
        send_stream: SendStream,
        recv_stream: RecvStream,
    ) -> Self {
        Self {
            stream_id,
            method,
            connection,
            send_stream: Arc::new(Mutex::new(Some(send_stream))),
            recv_stream: Arc::new(Mutex::new(Some(recv_stream))),
            is_active: Arc::new(AtomicBool::new(true)),
        }
    }

    /// ストリーム稼働状態の確認
    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }
}

/// Typed フレーム受信結果
pub enum TypedFrame {
    /// ProtocolMessage フレーム (type tag 0x00)
    Protocol(ProtocolMessage),
    /// Raw bytes フレーム (type tag 0x01)
    Raw(Vec<u8>),
}

impl UnisonStream {
    /// ProtocolMessage を typed フレームとして送信（type tag 0x00）
    ///
    /// SystemStream::send() を経由せず、ProtocolMessage → into_frame() → write_typed_frame() で
    /// type tag 付き length-prefixed フレームとして送信する。チャネル通信で使用。
    pub async fn send_frame(&self, msg: &ProtocolMessage) -> Result<(), NetworkError> {
        if !self.is_active() {
            return Err(NetworkError::Connection("Stream is not active".to_string()));
        }

        let frame = msg.clone().into_frame()?;
        let frame_bytes = frame.to_bytes();

        let mut send_guard = self.send_stream.lock().await;
        if let Some(send_stream) = send_guard.as_mut() {
            write_typed_frame(send_stream, FRAME_TYPE_PROTOCOL, &frame_bytes)
                .await
                .map_err(|e| NetworkError::Quic(format!("Failed to send frame: {}", e)))?;
            Ok(())
        } else {
            Err(NetworkError::Connection(
                "Send stream is closed".to_string(),
            ))
        }
    }

    /// Raw bytes を typed フレームとして送信（type tag 0x01）
    ///
    /// rkyv/zstd をバイパスし、length-prefix + type tag + raw payload のみ。
    /// オーディオストリーミング等の最小オーバーヘッド通信に使用。
    pub async fn send_raw_frame(&self, data: &[u8]) -> Result<(), NetworkError> {
        if !self.is_active() {
            return Err(NetworkError::Connection("Stream is not active".to_string()));
        }

        let mut send_guard = self.send_stream.lock().await;
        if let Some(send_stream) = send_guard.as_mut() {
            write_typed_frame(send_stream, FRAME_TYPE_RAW, data)
                .await
                .map_err(|e| NetworkError::Quic(format!("Failed to send raw frame: {}", e)))?;
            Ok(())
        } else {
            Err(NetworkError::Connection(
                "Send stream is closed".to_string(),
            ))
        }
    }

    /// ストリームを閉じる（&self で呼べるバージョン、Arc 共有時に使用）
    pub async fn close_stream(&self) -> Result<(), NetworkError> {
        self.is_active.store(false, Ordering::SeqCst);

        if let Some(mut send_stream) = self.send_stream.lock().await.take() {
            send_stream
                .finish()
                .map_err(|e| NetworkError::Quic(format!("Failed to close send stream: {}", e)))?;
        }

        if let Some(mut recv_stream) = self.recv_stream.lock().await.take() {
            recv_stream.stop(quinn::VarInt::from_u32(0)).map_err(|e| {
                NetworkError::Quic(format!("Failed to close receive stream: {}", e))
            })?;
        }

        info!(
            "Stream {} closed for method '{}'",
            self.stream_id, self.method
        );
        Ok(())
    }

    /// ProtocolMessage のみを受信（後方互換）
    ///
    /// typed frame を読んで ProtocolMessage のみを返す。
    /// Raw bytes フレームが来た場合はエラーを返す。
    pub async fn recv_frame(&self) -> Result<ProtocolMessage, NetworkError> {
        match self.recv_typed_frame().await? {
            TypedFrame::Protocol(msg) => Ok(msg),
            TypedFrame::Raw(_) => Err(NetworkError::Protocol(
                "Expected protocol frame, got raw bytes".to_string(),
            )),
        }
    }

    /// Typed フレームを受信（ProtocolMessage or Raw bytes）
    ///
    /// type tag で振り分けて TypedFrame を返す。
    /// チャネルの recv ループで使用し、Protocol/Raw を適切なキューに振り分ける。
    pub async fn recv_typed_frame(&self) -> Result<TypedFrame, NetworkError> {
        if !self.is_active() {
            return Err(NetworkError::Connection("Stream is not active".to_string()));
        }

        let mut recv_guard = self.recv_stream.lock().await;
        if let Some(recv_stream) = recv_guard.as_mut() {
            let (frame_type, payload) = read_typed_frame(recv_stream).await.map_err(|e| {
                self.is_active.store(false, Ordering::SeqCst);
                NetworkError::Quic(format!("Failed to read frame: {}", e))
            })?;

            match frame_type {
                FRAME_TYPE_PROTOCOL => {
                    let frame = ProtocolFrame::from_bytes(&payload)?;
                    let message = ProtocolMessage::from_frame(&frame)?;
                    Ok(TypedFrame::Protocol(message))
                }
                FRAME_TYPE_RAW => Ok(TypedFrame::Raw(payload.to_vec())),
                _ => Err(NetworkError::Protocol(format!(
                    "Unknown frame type tag: 0x{:02x}",
                    frame_type
                ))),
            }
        } else {
            Err(NetworkError::Connection(
                "Receive stream is closed".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::MessageType;

    /// ヘルパー: テスト用の ProtocolMessage を作成
    fn make_message(method: &str) -> ProtocolMessage {
        ProtocolMessage {
            id: 1,
            method: method.to_string(),
            msg_type: MessageType::Event,
            payload: b"{}".to_vec(),
        }
    }

    /// identity メッセージ ("__identity") が oneshot チャネルにルーティングされ、
    /// mpsc チャネルには流れないことを検証する。
    #[tokio::test]
    async fn test_identity_message_routed_to_oneshot() {
        let (mpsc_tx, mut mpsc_rx) = mpsc::unbounded_channel::<ProtocolMessage>();
        let (id_tx, id_rx) = oneshot::channel::<ProtocolMessage>();
        let identity_tx = Arc::new(Mutex::new(Some(id_tx)));

        let msg = make_message("__identity");

        // client_accept_bi_loop 内の分岐ロジックを再現
        if msg.method == "__identity" {
            if let Some(tx) = identity_tx.lock().await.take() {
                let _ = tx.send(msg);
            }
        } else {
            let _ = mpsc_tx.send(msg);
        }

        // oneshot で受信できること
        let received = id_rx.await.expect("oneshot から受信できるべき");
        assert_eq!(received.method, "__identity");

        // mpsc は空のままであること
        assert!(
            mpsc_rx.try_recv().is_err(),
            "mpsc チャネルは空のままであるべき"
        );
    }

    /// 非 identity メッセージ ("__channel:test") が mpsc チャネルにルーティングされることを検証する。
    #[tokio::test]
    async fn test_non_identity_message_routed_to_mpsc() {
        let (mpsc_tx, mut mpsc_rx) = mpsc::unbounded_channel::<ProtocolMessage>();
        let (id_tx, _id_rx) = oneshot::channel::<ProtocolMessage>();
        let identity_tx = Arc::new(Mutex::new(Some(id_tx)));

        let msg = make_message("__channel:test");

        // client_accept_bi_loop 内の分岐ロジックを再現
        if msg.method == "__identity" {
            if let Some(tx) = identity_tx.lock().await.take() {
                let _ = tx.send(msg);
            }
        } else {
            let _ = mpsc_tx.send(msg);
        }

        // mpsc で受信できること
        let received = mpsc_rx.try_recv().expect("mpsc から受信できるべき");
        assert_eq!(received.method, "__channel:test");
    }

    /// receive_identity() が指定時間内に応答がない場合タイムアウトエラーを返すことを検証する。
    #[tokio::test]
    async fn test_receive_identity_timeout() {
        let client = QuicClient::new().expect("QuicClient::new() は成功するべき");

        // oneshot の rx をセット（sender は保持するが送信しない）
        let (id_tx, id_rx) = oneshot::channel::<ProtocolMessage>();
        *client.identity_rx.lock().await = Some(id_rx);

        let result = client
            .receive_identity(std::time::Duration::from_millis(50))
            .await;

        assert!(result.is_err(), "タイムアウトでエラーになるべき");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("timed out"),
            "タイムアウトエラーメッセージを含むべき: {}",
            err_msg
        );

        // id_tx を drop して oneshot の sender 側を解放
        drop(id_tx);
    }

    /// receive_identity() を2回呼んだとき、2回目は "already consumed" エラーを返すことを検証する。
    #[tokio::test]
    async fn test_receive_identity_already_consumed() {
        let client = QuicClient::new().expect("QuicClient::new() は成功するべき");

        // oneshot チャネルを作成し、即座にメッセージを送信
        let (id_tx, id_rx) = oneshot::channel::<ProtocolMessage>();
        *client.identity_rx.lock().await = Some(id_rx);

        let msg = make_message("__identity");
        id_tx.send(msg).expect("oneshot 送信は成功するべき");

        // 1回目: 正常に受信
        let first = client
            .receive_identity(std::time::Duration::from_millis(100))
            .await;
        assert!(first.is_ok(), "1回目の receive_identity は成功するべき");
        assert_eq!(first.unwrap().method, "__identity");

        // 2回目: already consumed エラー
        let second = client
            .receive_identity(std::time::Duration::from_millis(100))
            .await;
        assert!(
            second.is_err(),
            "2回目の receive_identity はエラーになるべき"
        );
        let err_msg = second.unwrap_err().to_string();
        assert!(
            err_msg.contains("already consumed"),
            "already consumed エラーメッセージを含むべき: {}",
            err_msg
        );
    }

    // ─────────────────────────────────────────
    // resolve_socket_addr — IPv4 / IPv6 / DNS hostname tests
    // ─────────────────────────────────────────

    #[tokio::test]
    async fn resolve_ipv6_literal_with_port() {
        let sa = resolve_socket_addr("[::1]:8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_ipv6_literal_without_port_uses_default() {
        let sa = resolve_socket_addr("::1").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), DEFAULT_PORT);
    }

    #[tokio::test]
    async fn resolve_ipv4_literal_with_port_is_now_supported() {
        let sa = resolve_socket_addr("127.0.0.1:8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V4(_)));
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_port_only_falls_back_to_ipv6_loopback() {
        let sa = resolve_socket_addr("8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_localhost_with_port_via_dns() {
        let sa = resolve_socket_addr("localhost:8080").await.unwrap();
        // tokio::net::lookup_host が IPv4 / IPv6 のどちらを返すかは環境依存だが
        // port は確実に 8080
        assert_eq!(sa.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_strips_https_scheme() {
        let sa = resolve_socket_addr("https://[::1]:4510").await.unwrap();
        assert!(matches!(sa, SocketAddr::V6(_)));
        assert_eq!(sa.port(), 4510);
    }

    #[tokio::test]
    async fn resolve_strips_http_scheme() {
        let sa = resolve_socket_addr("http://127.0.0.1:8080").await.unwrap();
        assert!(matches!(sa, SocketAddr::V4(_)));
    }

    #[tokio::test]
    async fn resolve_strips_quic_scheme() {
        let sa = resolve_socket_addr("quic://[::1]:9999").await.unwrap();
        assert_eq!(sa.port(), 9999);
    }

    #[tokio::test]
    async fn resolve_unresolvable_hostname_errors() {
        let res = resolve_socket_addr("definitely-not-a-real-host-12345.invalid:8080").await;
        assert!(res.is_err(), "unresolvable hostname should error");
    }

    #[test]
    fn has_port_recognizes_ipv4_with_port() {
        assert!(has_port("127.0.0.1:8080"));
        assert!(has_port("example.com:443"));
    }

    #[test]
    fn has_port_recognizes_ipv6_bracket_with_port() {
        assert!(has_port("[::1]:8080"));
        assert!(has_port("[fd7a:115c:a1e0::f936:d97b]:4510"));
    }

    #[test]
    fn has_port_rejects_bare_ipv6_without_brackets() {
        // "::1" は port 無しと扱う (IPv6 リテラルは bracket 必須)
        assert!(!has_port("::1"));
        assert!(!has_port("fd7a:115c:a1e0::f936:d97b"));
    }

    #[test]
    fn has_port_rejects_hostname_without_port() {
        assert!(!has_port("example.com"));
        assert!(!has_port("localhost"));
    }

    #[test]
    fn strip_scheme_removes_known_prefixes() {
        assert_eq!(strip_scheme("https://example.com:443"), "example.com:443");
        assert_eq!(strip_scheme("http://example.com:80"), "example.com:80");
        assert_eq!(strip_scheme("quic://example.com:4510"), "example.com:4510");
    }

    #[test]
    fn strip_scheme_keeps_address_when_no_prefix() {
        assert_eq!(strip_scheme("example.com:443"), "example.com:443");
        assert_eq!(strip_scheme("[::1]:8080"), "[::1]:8080");
    }
}
