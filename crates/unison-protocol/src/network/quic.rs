//! Raw QUIC transport — [`QuicClient`] / [`QuicServer`] と接続ハンドラー。
//!
//! typed-frame の wire I/O は [`super::frame`]、 handler-facing なストリーム型
//! [`UnisonStream`](super::stream::UnisonStream) は [`super::stream`] にある。

use anyhow::{Context, Result};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
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

use super::addr::{resolve_socket_addr, sni_server_name};
use super::stream::UnisonStream;

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
    /// 接続時にサーバー証明書を検証する方法。 [`builder`](Self::builder) で明示
    /// 指定する。 [`insecure_localhost`](Self::insecure_localhost) 経由の場合は
    /// `SkipVerification` (= 検証なし、 loopback 限定)。
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

    /// **サーバー証明書を検証しない** client を構築する (= dev / test 用)。
    ///
    /// [`TrustAnchors::SkipVerification`] を選ぶため、 接続先の証明書を一切検証
    /// しない。 その代わり [`connect`](Self::connect) は接続先を **loopback に制限**
    /// する (非 loopback は `Err`)。 loopback 以外へ繋ぐ、 あるいは検証したい場合は
    /// [`builder`](Self::builder) で [`TrustAnchors`] を明示すること。
    ///
    /// 名前が `insecure` なのは意図的で、 呼び出し側のコードを読んだだけで
    /// 「ここは検証していない」 と分かるようにしている。
    ///
    /// [`TrustAnchors`]: crate::network::trust::TrustAnchors
    /// [`TrustAnchors::SkipVerification`]: crate::network::trust::TrustAnchors::SkipVerification
    pub fn insecure_localhost() -> Result<Self> {
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

        self.adopt_connection(endpoint, connection).await;
        Ok(())
    }

    /// 確立済み QUIC connection を client state に採用する（endpoint 保存・accept_bi
    /// loop 起動・identity oneshot 準備）。[`connect`](Self::connect) /
    /// [`connect_race`](Self::connect_race) 共通の後処理。
    async fn adopt_connection(&self, endpoint: Endpoint, connection: Connection) {
        info!(
            "Connected to QUIC server at {}",
            connection.remote_address()
        );

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
    }

    /// 複数の direct 候補アドレスへ Happy Eyeballs v2 の staggered race で接続する
    /// (ADR-020 §S6)。1 個の client Endpoint から候補を stagger 付きで並行 connect し、
    /// 最初に握手完了した経路を採用、残りは cancel する。「全滅」を判定せず、死経路の
    /// コストは stagger 1 tick で有界。
    ///
    /// **direct-first-cut**: IPv6 GUA (ADR-020 §D3-a = first-class direct) のみ race する。
    /// IPv4 候補は doctrine 上まだ deferred (§D3) のため **warn して skip**（silent drop
    /// はしない）。relay fallback は engine 側 relay-ready だが本メソッドでは未配線
    /// (relay は別到達機構ゆえ Transport 抽象が要る = 次段)。
    ///
    /// `server_name` は全候補共通（= 同一 world の cert 名前検証に使う）。
    pub async fn connect_race(
        &self,
        addrs: Vec<SocketAddr>,
        server_name: &str,
        cfg: super::dial::RaceCfg,
    ) -> Result<()> {
        use super::dial::{AttemptOutcome, Candidate, Via};

        // IPv6 GUA のみ採用。IPv4 は §D3 で deferred ゆえ明示 skip（silent drop なし）。
        let mut v6: Vec<SocketAddr> = Vec::new();
        for a in addrs {
            if a.is_ipv6() {
                v6.push(a);
            } else {
                warn!("connect_race: IPv4 候補 {a} を skip (ADR-020 §D3: IPv4 deferred)");
            }
        }
        if v6.is_empty() {
            return Err(anyhow::anyhow!(
                "connect_race: IPv6 direct 候補がありません"
            ));
        }

        // SkipVerification は loopback のみ許可（connect と対称）。
        if matches!(
            self.trust_anchors,
            super::trust::TrustAnchors::SkipVerification
        ) && let Some(a) = v6.iter().find(|a| !a.ip().is_loopback())
        {
            return Err(anyhow::anyhow!(
                "SkipVerification is restricted to loopback; got {a}"
            ));
        }

        let client_config = Self::configure_client_with(self.trust_anchors.clone()).await?;

        // v6 候補を 1 個の [::] endpoint から race する（複数 connect を同時発火）。
        let mut endpoint = Endpoint::client("[::]:0".parse().unwrap())?;
        endpoint.set_default_client_config(client_config);

        let candidates: Vec<Candidate> = v6.into_iter().map(Candidate::Direct).collect();

        let winner = super::dial::race::<Connection, _, _>(candidates, cfg, |cand| {
            // `&endpoint` を future に move（endpoint 本体は race 後に adopt するため保持）。
            let ep = &endpoint;
            async move {
                let addr = match &cand {
                    Candidate::Direct(a) => *a,
                    // relay は direct-first-cut では未配線（次段の Transport 抽象で）。
                    Candidate::Relay(_) => return AttemptOutcome::Failed { via: cand.via() },
                };
                let t0 = tokio::time::Instant::now();
                match ep.connect(addr, server_name) {
                    Ok(connecting) => match connecting.await {
                        Ok(conn) => AttemptOutcome::Connected {
                            transport: conn,
                            via: Via::Direct(addr),
                            rtt: t0.elapsed(),
                        },
                        Err(_) => AttemptOutcome::Failed {
                            via: Via::Direct(addr),
                        },
                    },
                    Err(_) => AttemptOutcome::Failed {
                        via: Via::Direct(addr),
                    },
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("connect_race failed: {e}"))?;

        self.adopt_connection(endpoint, winner.transport).await;
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
    /// [`QuicServer::builder`] で指定する。 未指定なら
    /// [`CertSource::dev_localhost`](super::cert::CertSource::dev_localhost) (= DEV ONLY)。
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
    /// [`QuicServer`] を組み立てる唯一の入口。
    pub fn builder(server: Arc<ProtocolServer>) -> QuicServerBuilder {
        QuicServerBuilder {
            server,
            cert_source: None,
        }
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

    /// 接続の待ち受けを開始する (= endpoint が閉じるまでブロック)。
    ///
    /// 停止したい場合は [`start_with_shutdown`](Self::start_with_shutdown)。
    pub async fn start(&self) -> Result<()> {
        // sender をこの関数のスコープに保持したまま await するので、 shutdown は発火しない。
        let (_never, rx) = tokio::sync::oneshot::channel();
        self.start_with_shutdown(rx).await
    }

    /// shutdown シグナルを受け付けながら待ち受ける。
    ///
    /// `shutdown_rx` が発火するか endpoint が閉じるまでループする。
    pub async fn start_with_shutdown(
        &self,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let endpoint = self
            .endpoint
            .as_ref()
            .context("Server not bound to an address")?;

        info!("QUIC server listening for connections");

        loop {
            tokio::select! {
                connecting = endpoint.accept() => {
                    match connecting {
                        Some(connecting) => {
                            // handshake 完了は per-connection task 内で行う (start() と同じ理由で
                            // accept loop を守る = 片肺死の根治、2026-07-13、mem_1CcvYA5TRF4EcFafbyKqPg)。
                            let server = Arc::clone(&self.server);
                            let ctx = Arc::new(ConnectionContext::new());
                            tokio::spawn(async move {
                                let connection = match connecting.await {
                                    Ok(c) => c,
                                    Err(e) => {
                                        warn!("QUIC handshake failed (accept loop 継続): {}", e);
                                        return;
                                    }
                                };
                                info!("New QUIC connection from: {}", connection.remote_address());
                                let conn: Arc<dyn UnisonConn> = Arc::new(connection);
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
        if msg.method == "__identity"
            && let Some(tx) = identity_tx.take()
        {
            let _ = tx.send(msg);
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
        let client = QuicClient::insecure_localhost()
            .expect("QuicClient::insecure_localhost() は成功するべき");

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
        let client = QuicClient::insecure_localhost()
            .expect("QuicClient::insecure_localhost() は成功するべき");

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
}
