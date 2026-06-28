//! Raw QUIC transport — [`QuicClient`] / [`QuicServer`] と接続ハンドラー。
//!
//! typed-frame の wire I/O は [`super::frame`]、 handler-facing なストリーム型
//! [`UnisonStream`] は [`super::stream`] に分離されている。 後方互換のため、
//! それらは本モジュールからも re-export されている (= `network::quic::*` の
//! import パスを維持)。

use anyhow::{Context, Result};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig as RustlsClientConfig, ServerConfig as RustlsServerConfig};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, oneshot};
use tracing::{error, info, warn};

use super::conn::UnisonConn;
use super::dispatch::{
    ClientServerChannelHandler, ClientServerChannelRegistry, client_accept_bi_loop,
    handle_connection,
};
use super::{NetworkError, ProtocolMessage, context::ConnectionContext, server::ProtocolServer};

// 後方互換: typed-frame wire I/O と handler-facing stream 型は専用モジュールへ
// 移動したが、 `network::quic::*` の public import パスを保つため再公開する。
pub use super::frame::{
    CHANNEL_ACK_METHOD, FRAME_TYPE_PROTOCOL, FRAME_TYPE_RAW, read_frame, read_typed_frame,
    write_frame, write_typed_frame,
};
pub use super::stream::{TypedFrame, UnisonStream};

/// Default certificate file paths for assets/certs directory
pub const DEFAULT_CERT_PATH: &str = "assets/certs/cert.pem";
pub const DEFAULT_KEY_PATH: &str = "assets/certs/private_key.der";

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

/// TLS SNI / 証明書検証に渡す server name を URL から抽出する。
///
/// `Endpoint::connect` の server_name は rustls の証明書 **名前検証** に使われる。
/// 以前はリテラル `"localhost"` 固定で、(a) mesh CA が実ホスト名で発行した
/// 証明書への接続が常に名前不一致で失敗し、(b) `localhost` SAN を持つ証明書なら
/// 任意のホストになりすませる name-pinning 不全を生んでいた。
///
/// 解決方針:
/// - hostname 入力 (`example.com:443`) → hostname を SNI に (= DNS SAN 照合)
/// - IP リテラル入力 (`[::1]:8080` / `127.0.0.1:8080` / `::1`) → IP 文字列を SNI に
///   (rustls は `ServerName::IpAddress` として IP SAN を照合)
/// - port のみ (`8080`) → 解決後の loopback IP を SNI に
fn sni_server_name(addr: &str, resolved: SocketAddr) -> String {
    let addr = strip_scheme(addr);

    // IPv6 bracket リテラル ("[::1]:8080" / "[::1]") → 括弧内の IP
    if let Some(rest) = addr.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        return rest[..end].to_string();
    }

    // port のみ ("8080") → 解決済み loopback IP
    if addr.parse::<u16>().is_ok() {
        return resolved.ip().to_string();
    }

    // bracket なし IPv6 リテラル ("::1", "fd7a:115c::1")
    if addr.contains(':') && !addr.contains('.') && addr.parse::<std::net::Ipv6Addr>().is_ok() {
        return addr.to_string();
    }

    // host:port → host 部分、port なし → そのまま (hostname or IPv4 リテラル)
    if has_port(addr)
        && let Some(colon) = addr.rfind(':')
    {
        return addr[..colon].to_string();
    }
    addr.to_string()
}

/// QUIC client implementation
pub struct QuicClient {
    endpoint: Mutex<Option<Endpoint>>,
    connection: Arc<RwLock<Option<Connection>>>,
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
    /// server-initiated channel (= `from="server"`) の handler registry。
    ///
    /// `client_accept_bi_loop` が server 発信 stream の先頭宣言 frame の method で引く。
    /// [`register_server_channel`](Self::register_server_channel) で登録。
    server_channels: ClientServerChannelRegistry,
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
        Ok(QuicClient {
            endpoint: Mutex::new(None),
            connection: Arc::new(RwLock::new(None)),
            identity_rx: Arc::new(Mutex::new(None)),
            identity_tx: Arc::new(Mutex::new(None)),
            response_tasks: Arc::new(Mutex::new(Vec::new())),
            trust_anchors,
            server_channels: Arc::new(RwLock::new(HashMap::new())),
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

    /// 証明書検証を行わない insecure な client を構築する。
    ///
    /// **注意**: この constructor は [`TrustAnchors::SkipVerification`] を
    /// 暗黙に選択するため、サーバー証明書を一切検証しない。 production では
    /// [`QuicClient::builder`] で明示的に [`TrustAnchors`] を指定すること。
    /// なお [`QuicClient::connect`] は SkipVerification 時の接続先を loopback に
    /// 制限する。
    ///
    /// [`TrustAnchors`]: crate::network::trust::TrustAnchors
    /// [`TrustAnchors::SkipVerification`]: crate::network::trust::TrustAnchors::SkipVerification
    pub fn new() -> Result<Self> {
        warn!(
            "QuicClient::new() constructs an INSECURE client (no server certificate \
             verification). Use QuicClient::builder() with an explicit TrustAnchors for \
             the secure path."
        );
        Ok(Self {
            endpoint: Mutex::new(None),
            connection: Arc::new(RwLock::new(None)),
            identity_rx: Arc::new(Mutex::new(None)),
            identity_tx: Arc::new(Mutex::new(None)),
            response_tasks: Arc::new(Mutex::new(Vec::new())),
            trust_anchors: super::trust::TrustAnchors::SkipVerification,
            server_channels: Arc::new(RwLock::new(HashMap::new())),
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
        // v0.9.0: enable QUIC datagrams (= unreliable / unordered, ≤MTU). Used by
        // [`QuicClient::send_datagram`] / [`QuicClient::recv_datagram`] for high-
        // frequency low-overhead broadcasts (e.g. 3DCG transform sync). 1300B is
        // the safe MTU upper bound (= 1500 - IP/UDP/QUIC header).
        transport_config.datagram_receive_buffer_size(Some(1024 * 1024));
        transport_config.datagram_send_buffer_size(1024 * 1024);
        client_config.transport_config(Arc::new(transport_config));

        Ok(client_config)
    }

    // 双方向ストリームを使うため、start_receive_loopは不要になりました

    /// QUIC接続への参照を取得（チャネル用ストリーム開設に使用）
    pub fn connection(&self) -> &Arc<RwLock<Option<Connection>>> {
        &self.connection
    }

    /// server-initiated channel (= `from="server"`) の handler を登録する。
    ///
    /// server が [`ConnectionContext::open_server_stream`](super::context::ConnectionContext::open_server_stream)
    /// で開いた stream の先頭宣言 frame の method がこの `channel` と一致すると、handler に
    /// raw [`UnisonStream`] が渡る。handler はその stream を **直読** して reliable に payload
    /// を受ける（recv ループ／中継 mpsc を挟まない = 取りこぼし無し）。
    ///
    /// `connect` 前に登録しておくこと（接続直後に届く server-initiated stream を取りこぼさない）。
    /// 同 `channel` で再登録すると古い handler を replace する。
    pub async fn register_server_channel<F, Fut>(&self, channel: &str, handler: F)
    where
        F: Fn(UnisonStream) -> Fut + Send + Sync + 'static,
        Fut: futures_util::Future<Output = Result<(), NetworkError>> + Send + 'static,
    {
        let handler: ClientServerChannelHandler = Arc::new(move |stream: UnisonStream| {
            Box::pin(handler(stream))
                as Pin<Box<dyn futures_util::Future<Output = Result<(), NetworkError>> + Send>>
        });
        self.server_channels
            .write()
            .await
            .insert(channel.to_string(), handler);
    }
}

impl QuicClient {
    /// サーバーアドレスを解析 (IPv4 / IPv6 / DNS hostname 対応)
    async fn parse_server_address(addr: &str) -> Result<SocketAddr> {
        resolve_socket_addr(addr).await
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

        // SkipVerification は loopback 接続にのみ許可する (TS 側 `enforceTrustGate`
        // と対称)。 任意のホストに対する証明書検証スキップを防ぐ。
        if matches!(
            self.trust_anchors,
            super::trust::TrustAnchors::SkipVerification
        ) && !addr.ip().is_loopback()
        {
            return Err(anyhow::anyhow!(
                "SkipVerification is restricted to loopback; got {} (resolved from {}). \
                 Use QuicClient::builder() with an explicit TrustAnchors to connect to \
                 non-loopback hosts.",
                addr,
                url
            ));
        }

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

        // SNI / 証明書名前検証に渡す server name を実 URL から導出する
        // (旧実装は "localhost" 固定で名前検証が機能していなかった)。
        let server_name = sni_server_name(url, addr);
        let connection = endpoint
            .connect(addr, &server_name)?
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
        let identity_tx = self.identity_tx.clone();
        let server_channels = Arc::clone(&self.server_channels);
        let task = tokio::spawn(async move {
            client_accept_bi_loop(connection_for_loop, identity_tx, server_channels).await;
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

    /// Send a single QUIC datagram (= **unreliable / unordered, ≤MTU**).
    ///
    /// v0.9.0 で MVP として thin wrapper を expose。 channel 抽象を経由しない
    /// connection-level API、 caller は payload 自体に必要な header (= channel ID
    /// 等の demux 情報) を含める責任を持つ。
    ///
    /// # 用途想定
    ///
    /// - 3DCG position+rotation transform の高頻度 broadcast (= 60Hz / 120Hz、
    ///   1 frame で大量配信、 古いは新しいで上書き)
    /// - low-latency event push (= ack 不要、 fire-and-forget)
    /// - heartbeat / presence
    ///
    /// # Size limit
    ///
    /// 安全 MTU (= IP MTU 1500 - IP/UDP/QUIC header ≈ 1300B) 以下を推奨。 超過
    /// すると `SendDatagramError::TooLarge` が返り、 sender 側 fragment 不可。
    ///
    /// # 信頼性
    ///
    /// 配送保証なし、 順序保証なし。 reliable / ordered が必要なら channel API
    /// (= `open_channel`) を使う。
    ///
    /// # Channel 統合 (v0.10+)
    ///
    /// 現状は connection 単位 raw datagram。 v0.10+ で `event "X" backend="datagram"`
    /// KDL schema 拡張と一緒に channel API へ統合予定 (= `design/wire-format.md`
    /// 参照)。
    pub async fn send_datagram(&self, data: bytes::Bytes) -> Result<()> {
        let connection_guard = self.connection.read().await;
        let connection = connection_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("send_datagram: not connected"))?;
        connection
            .send_datagram(data)
            .map_err(|e| anyhow::anyhow!("send_datagram failed: {}", e))
    }

    /// Receive the next QUIC datagram (blocks until one arrives or connection closes).
    ///
    /// pair API for [`Self::send_datagram`]. v0.9.0 では caller が任意の demux 戦略
    /// を実装する (= channel ID prefix 等を payload 内に持つ)。
    pub async fn recv_datagram(&self) -> Result<bytes::Bytes> {
        let connection_guard = self.connection.read().await;
        let connection = connection_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("recv_datagram: not connected"))?;
        connection
            .read_datagram()
            .await
            .map_err(|e| anyhow::anyhow!("recv_datagram failed: {}", e))
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

        let private_key = PrivateKeyDer::try_from(private_key_der_bytes)
            .map_err(|e| anyhow::anyhow!("Failed to wrap private key: {}", e))?;

        Ok((vec![CertificateDer::from(cert_der_bytes)], private_key))
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
        let mut rustls_server_config = RustlsServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleCertResolver(certified_key)));

        // QUIC は ALPN 必須 (RFC 9001 §8.1)。 client (trust.rs) と同一 label で
        // 合意する。 SSOT は `super::UNISON_ALPN`。
        rustls_server_config.alpn_protocols = vec![super::UNISON_ALPN.to_vec()];

        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(rustls_server_config)?;
        let mut server_config = ServerConfig::with_crypto(Arc::new(crypto));

        let mut transport_config = quinn::TransportConfig::default();
        transport_config
            .max_idle_timeout(Some(std::time::Duration::from_secs(60).try_into().unwrap()));
        transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(10)));
        transport_config.max_concurrent_uni_streams(0u32.into());
        transport_config.max_concurrent_bidi_streams(1000u32.into());
        transport_config.initial_rtt(std::time::Duration::from_millis(100));
        // v0.9.0: enable QUIC datagrams (= same as client side、 server-initiated
        // broadcast 用 e.g. 3DCG transform sync from server)
        transport_config.datagram_receive_buffer_size(Some(1024 * 1024));
        transport_config.datagram_send_buffer_size(1024 * 1024);
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
            let conn: Arc<dyn UnisonConn> = Arc::new(connection);
            tokio::spawn(async move {
                if let Err(e) = handle_connection(conn, server, ctx).await {
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
                            let conn: Arc<dyn UnisonConn> = Arc::new(connection);
                            tokio::spawn(async move {
                                if let Err(e) = handle_connection(conn, server, ctx).await {
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

    /// client_accept_bi_loop の分岐ロジック (= identity_tx を消費するか否か) を再現。
    /// `__identity` のみ oneshot を消費し、 それ以外は drop されること (= 旧 mpsc 経路撤去)。
    fn route_like_accept_bi_loop(
        method: &str,
        identity_tx: &mut Option<oneshot::Sender<ProtocolMessage>>,
    ) {
        let msg = make_message(method);
        if msg.method == "__identity" {
            if let Some(tx) = identity_tx.take() {
                let _ = tx.send(msg);
            }
        }
        // else: server-initiated 非 identity frame は drop (= 何もしない)
    }

    /// identity メッセージ ("__identity") が oneshot にルーティングされることを検証する。
    #[tokio::test]
    async fn test_identity_message_routed_to_oneshot() {
        let (id_tx, id_rx) = oneshot::channel::<ProtocolMessage>();
        let mut identity_tx = Some(id_tx);

        route_like_accept_bi_loop("__identity", &mut identity_tx);

        let received = id_rx.await.expect("oneshot から受信できるべき");
        assert_eq!(received.method, "__identity");
    }

    /// 非 identity メッセージは drop され、 identity oneshot を消費しないことを検証する。
    #[tokio::test]
    async fn test_non_identity_message_is_dropped() {
        let (id_tx, id_rx) = oneshot::channel::<ProtocolMessage>();
        let mut identity_tx = Some(id_tx);

        route_like_accept_bi_loop("__channel:test", &mut identity_tx);

        // identity oneshot は未消費のまま (= 非 identity は drop された)
        assert!(
            identity_tx.is_some(),
            "非 identity メッセージは identity oneshot を消費すべきでない"
        );
        drop(identity_tx);
        // sender が drop されたので rx は Err（送信されていない）
        assert!(id_rx.await.is_err(), "identity は送信されていないべき");
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

    // ─────────────────────────────────────────
    // sni_server_name — SNI / 証明書名前検証に渡す名前の導出
    // ─────────────────────────────────────────

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn sni_hostname_uses_hostname_not_resolved_ip() {
        // hostname 入力は (DNS 解決後の IP ではなく) hostname を SNI にする
        assert_eq!(
            sni_server_name("example.com:443", sa("93.184.216.34:443")),
            "example.com"
        );
        assert_eq!(
            sni_server_name("localhost:8080", sa("127.0.0.1:8080")),
            "localhost"
        );
    }

    #[test]
    fn sni_ipv6_bracket_literal_uses_ip() {
        assert_eq!(sni_server_name("[::1]:8080", sa("[::1]:8080")), "::1");
        assert_eq!(
            sni_server_name("[fd7a:115c::1]:4510", sa("[fd7a:115c::1]:4510")),
            "fd7a:115c::1"
        );
    }

    #[test]
    fn sni_bare_ipv6_literal_uses_ip() {
        assert_eq!(sni_server_name("::1", sa("[::1]:8080")), "::1");
    }

    #[test]
    fn sni_ipv4_literal_uses_ip() {
        assert_eq!(
            sni_server_name("127.0.0.1:8080", sa("127.0.0.1:8080")),
            "127.0.0.1"
        );
    }

    #[test]
    fn sni_port_only_uses_resolved_loopback_ip() {
        assert_eq!(sni_server_name("8080", sa("[::1]:8080")), "::1");
    }

    #[test]
    fn sni_strips_scheme_first() {
        assert_eq!(
            sni_server_name("quic://example.com:4510", sa("10.0.0.1:4510")),
            "example.com"
        );
        assert_eq!(
            sni_server_name("https://[::1]:4510", sa("[::1]:4510")),
            "::1"
        );
    }

    #[test]
    fn sni_is_never_hardcoded_localhost() {
        // regression guard: 旧実装の "localhost" 固定に戻らないこと
        assert_ne!(
            sni_server_name("cp.fleetstage.cloud:4510", sa("10.1.2.3:4510")),
            "localhost"
        );
    }
}
