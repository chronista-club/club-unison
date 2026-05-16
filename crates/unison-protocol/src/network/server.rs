use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::codec::{Codec, Encodable, JsonCodec};

use super::NetworkError;
use super::datagram_channel::{DatagramChannel, encode_varint};
use super::identity::{ChannelDirection, ChannelInfo, ChannelStatus, ServerIdentity};

/// 接続イベント通知
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    /// 新しい接続が確立された
    Connected {
        remote_addr: SocketAddr,
        context: Arc<super::context::ConnectionContext>,
    },
    /// 接続が切断された
    Disconnected { remote_addr: SocketAddr },
}

/// [`ProtocolServer::subscribe_connection_events()`] が返す接続イベントレシーバー
///
/// `tokio::sync::broadcast::Receiver<ConnectionEvent>` のラッパーで、
/// [`RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
/// を透過的にスキップするヘルパーメソッドを提供する。
pub struct ConnectionEventReceiver {
    inner: tokio::sync::broadcast::Receiver<ConnectionEvent>,
}

impl ConnectionEventReceiver {
    /// 内部の broadcast::Receiver を直接参照する
    pub fn inner(&mut self) -> &mut tokio::sync::broadcast::Receiver<ConnectionEvent> {
        &mut self.inner
    }

    /// 次のイベントを受信する（`broadcast::Receiver::recv()` の委譲）
    ///
    /// `Lagged` エラーが発生した場合はそのまま呼び出し元に返す。
    /// `Lagged` を自動スキップしたい場合は [`recv_skip_lagged()`](Self::recv_skip_lagged) を使用する。
    pub async fn recv(
        &mut self,
    ) -> Result<ConnectionEvent, tokio::sync::broadcast::error::RecvError> {
        self.inner.recv().await
    }

    /// 次のイベントを受信する（`Lagged` エラー時は自動スキップして最新から再開）
    ///
    /// subscriber の処理が遅延し、バッファの古いイベントが上書きされた場合
    /// (`RecvError::Lagged`)、エラーを無視して最新のイベントから受信を再開する。
    ///
    /// チャネルが閉じられた場合は `RecvError::Closed` を返す。
    ///
    /// # 例
    ///
    /// ```rust,no_run
    /// # use unison::ProtocolServer;
    /// # async fn example(server: &ProtocolServer) {
    /// let mut rx = server.subscribe_connection_events();
    /// loop {
    ///     match rx.recv_skip_lagged().await {
    ///         Ok(event) => { /* イベント処理 */ }
    ///         Err(_closed) => break,
    ///     }
    /// }
    /// # }
    /// ```
    pub async fn recv_skip_lagged(
        &mut self,
    ) -> Result<ConnectionEvent, tokio::sync::broadcast::error::RecvError> {
        let mut total_skipped: u64 = 0;
        loop {
            match self.inner.recv().await {
                Ok(event) => return Ok(event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    total_skipped += n;
                    tracing::warn!(
                        skipped = n,
                        total_skipped,
                        "接続イベントが {} 件遅延、スキップして最新から再開（累計: {} 件）",
                        n,
                        total_skipped
                    );
                    continue;
                }
                Err(e @ tokio::sync::broadcast::error::RecvError::Closed) => return Err(e),
            }
        }
    }
}

/// チャネルハンドラー型（接続コンテキスト + UnisonStreamを受け取る）
pub type ChannelHandler = Arc<
    dyn Fn(
            Arc<super::context::ConnectionContext>,
            super::quic::UnisonStream,
        ) -> Pin<Box<dyn futures_util::Future<Output = Result<(), NetworkError>> + Send>>
        + Send
        + Sync,
>;

/// Datagram channel handler 型 (v0.10.0 で追加)
///
/// 接続ごとに一度だけ invoke される。 `DatagramChannel<JsonCodec>` を受け取り、
/// 内部で `recv_event` / `send_event` の loop を回すのが典型。
pub type DatagramChannelHandler = Arc<
    dyn Fn(DatagramChannel<JsonCodec>) -> Pin<Box<dyn futures_util::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Datagram channel の registry エントリ (= name に紐づく channel_id + handler)
pub(crate) struct DatagramHandlerEntry {
    pub(crate) channel_id: u64,
    pub(crate) handler: DatagramChannelHandler,
}

/// サーバーのライフサイクルを管理するハンドル
///
/// `spawn_listen()` が返す。shutdown シグナル送信と完了待ちを提供。
pub struct ServerHandle {
    join_handle: JoinHandle<Result<(), NetworkError>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    local_addr: SocketAddr,
}

impl ServerHandle {
    /// サーバーをグレースフルにシャットダウンし、完了を待つ
    pub async fn shutdown(mut self) -> Result<(), NetworkError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.join_handle
            .await
            .map_err(|e| NetworkError::Quic(format!("Server task panicked: {}", e)))?
    }

    /// サーバータスクが終了済みかどうか
    pub fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }

    /// サーバーがバインドしたローカルアドレスを取得
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

/// プロトコルサーバー実装
pub struct ProtocolServer {
    running: Arc<AtomicBool>,
    /// サーバー識別情報
    server_name: String,
    server_version: String,
    server_namespace: String,
    /// チャネルハンドラー（チャネル名 → ハンドラー関数）
    channel_handlers: Arc<RwLock<HashMap<String, ChannelHandler>>>,
    /// Datagram channel handlers (v0.10.0 で追加、 name → channel_id + handler)
    datagram_channel_handlers: Arc<RwLock<HashMap<String, DatagramHandlerEntry>>>,
    /// Active connections (= broadcast 配信先、 remote_addr → Connection)
    active_connections: Arc<RwLock<HashMap<SocketAddr, Arc<quinn::Connection>>>>,
    /// 接続イベント broadcast チャネル（複数サブスクライバ対応）
    connection_event_tx: tokio::sync::broadcast::Sender<ConnectionEvent>,
}

impl ProtocolServer {
    pub fn new() -> Self {
        // capacity 64: 接続イベント（Connected/Disconnected）は接続ライフサイクルに
        // 紐づく低頻度イベントのため、64 件のバッファで十分な余裕がある。
        // 仮に超過した場合は RecvError::Lagged が返り、recv_skip_lagged() で対処可能。
        let (tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            running: Arc::new(AtomicBool::new(false)),
            server_name: "unison".to_string(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            server_namespace: "default".to_string(),
            channel_handlers: Arc::new(RwLock::new(HashMap::new())),
            datagram_channel_handlers: Arc::new(RwLock::new(HashMap::new())),
            active_connections: Arc::new(RwLock::new(HashMap::new())),
            connection_event_tx: tx,
        }
    }

    /// サーバー識別情報を設定して作成
    pub fn with_identity(name: &str, version: &str, namespace: &str) -> Self {
        Self {
            server_name: name.to_string(),
            server_version: version.to_string(),
            server_namespace: namespace.to_string(),
            ..Self::new()
        }
    }

    /// サーバー実行状態の確認
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 登録済みチャネルからServerIdentityを構築
    pub async fn build_identity(&self) -> ServerIdentity {
        let mut identity = ServerIdentity::new(
            &self.server_name,
            &self.server_version,
            &self.server_namespace,
        );

        // チャネルハンドラーからChannelInfoを構築
        let handlers = self.channel_handlers.read().await;
        for channel_name in handlers.keys() {
            identity.add_channel(ChannelInfo {
                name: channel_name.clone(),
                direction: ChannelDirection::Bidirectional,
                lifetime: "persistent".to_string(),
                status: ChannelStatus::Available,
            });
        }

        identity
    }

    /// チャネルハンドラーを登録
    pub async fn register_channel<F, Fut>(&self, name: &str, handler: F)
    where
        F: Fn(Arc<super::context::ConnectionContext>, super::quic::UnisonStream) -> Fut
            + Send
            + Sync
            + 'static,
        Fut: futures_util::Future<Output = Result<(), NetworkError>> + Send + 'static,
    {
        let handler = Arc::new(
            move |ctx: Arc<super::context::ConnectionContext>,
                  stream: super::quic::UnisonStream| {
                Box::pin(handler(ctx, stream))
                    as Pin<Box<dyn futures_util::Future<Output = Result<(), NetworkError>> + Send>>
            },
        );

        let mut handlers = self.channel_handlers.write().await;
        handlers.insert(name.to_string(), handler);
    }

    /// Datagram channel handler を登録 (v0.10.0 で追加)
    ///
    /// `name` と `channel_id` (= KDL schema 由来) のペアで一意、 connection 毎に
    /// handler が一度 invoke される。 handler 内で `chan.recv_event` の loop を
    /// 回すのが典型。
    ///
    /// 同 name で再登録すると **古い entry を replace**。
    pub async fn register_channel_datagram<F, Fut>(&self, name: &str, channel_id: u64, handler: F)
    where
        F: Fn(DatagramChannel<JsonCodec>) -> Fut + Send + Sync + 'static,
        Fut: futures_util::Future<Output = ()> + Send + 'static,
    {
        let handler: DatagramChannelHandler = Arc::new(move |chan: DatagramChannel<JsonCodec>| {
            Box::pin(handler(chan)) as Pin<Box<dyn futures_util::Future<Output = ()> + Send>>
        });
        let mut handlers = self.datagram_channel_handlers.write().await;
        handlers.insert(
            name.to_string(),
            DatagramHandlerEntry {
                channel_id,
                handler,
            },
        );
    }

    /// 全 active connection に対して datagram channel event を broadcast (v0.10.0)
    ///
    /// `channel_name` から `channel_id` を解決、 event を encode して varint prefix を
    /// 付け、 active な全 connection の `send_datagram` を呼ぶ。
    ///
    /// 戻り値は配送成功した connection 数 (= datagram は best-effort なので失敗は warn log
    /// のみで継続)。
    pub async fn broadcast<T, C>(
        &self,
        channel_name: &str,
        event: &T,
    ) -> Result<usize, NetworkError>
    where
        T: Encodable<C>,
        C: Codec,
    {
        let channel_id = {
            let handlers = self.datagram_channel_handlers.read().await;
            handlers
                .get(channel_name)
                .map(|entry| entry.channel_id)
                .ok_or_else(|| NetworkError::HandlerNotFound {
                    method: format!("datagram channel: {}", channel_name),
                })?
        };

        let encoded = event.encode().map_err(NetworkError::Codec)?;
        let mut payload =
            Vec::with_capacity(super::datagram_channel::VARINT_MAX_LEN + encoded.len());
        encode_varint(channel_id, &mut payload);
        payload.extend_from_slice(&encoded);

        let connections = self.active_connections.read().await;
        let mut success = 0usize;
        for connection in connections.values() {
            match connection.send_datagram(payload.clone().into()) {
                Ok(()) => success += 1,
                Err(e) => {
                    tracing::debug!(
                        "Broadcast to {}: send_datagram failed: {} (continuing)",
                        connection.remote_address(),
                        e
                    );
                }
            }
        }
        Ok(success)
    }

    /// Datagram handler の snapshot を取得 (= quic.rs::handle_connection 用、 内部 API)
    pub(crate) async fn snapshot_datagram_handlers(
        &self,
    ) -> Vec<(String, u64, DatagramChannelHandler)> {
        let handlers = self.datagram_channel_handlers.read().await;
        handlers
            .iter()
            .map(|(name, entry)| (name.clone(), entry.channel_id, Arc::clone(&entry.handler)))
            .collect()
    }

    /// Active connection を登録 (= quic.rs::handle_connection 用、 内部 API)
    pub(crate) async fn add_active_connection(
        &self,
        remote_addr: SocketAddr,
        connection: Arc<quinn::Connection>,
    ) {
        self.active_connections
            .write()
            .await
            .insert(remote_addr, connection);
    }

    /// Active connection を解除 (= quic.rs::handle_connection 用、 内部 API)
    pub(crate) async fn remove_active_connection(&self, remote_addr: SocketAddr) {
        self.active_connections.write().await.remove(&remote_addr);
    }

    /// Active connection 数 (= 主に test / debug 用)
    pub async fn active_connection_count(&self) -> usize {
        self.active_connections.read().await.len()
    }

    /// 接続イベントを購読する
    ///
    /// 接続/切断時に [`ConnectionEvent`] を受信できる。
    /// 複数のサブスクライバが同時に購読可能。
    ///
    /// # Lagged エラーについて
    ///
    /// 内部は `tokio::sync::broadcast` チャネル（capacity: 64）で実装されている。
    /// subscriber の消費が追いつかず、バッファが一巡すると古いイベントが破棄され、
    /// 次回の `recv()` で [`RecvError::Lagged(n)`](tokio::sync::broadcast::error::RecvError::Lagged)
    /// が返る（`n` はスキップされたイベント数）。
    ///
    /// `Lagged` を手動でハンドリングする場合は [`ConnectionEventReceiver::recv()`] を使用し、
    /// 自動的にスキップして最新から再開したい場合は
    /// [`ConnectionEventReceiver::recv_skip_lagged()`] を使用する。
    ///
    /// # 例
    ///
    /// ```rust,no_run
    /// # use unison::ProtocolServer;
    /// # async fn example(server: &ProtocolServer) {
    /// let mut rx = server.subscribe_connection_events();
    ///
    /// // Lagged を自動スキップして受信
    /// match rx.recv_skip_lagged().await {
    ///     Ok(event) => { /* イベント処理 */ }
    ///     Err(_closed) => { /* チャネル閉鎖 */ }
    /// }
    /// # }
    /// ```
    pub fn subscribe_connection_events(&self) -> ConnectionEventReceiver {
        ConnectionEventReceiver {
            inner: self.connection_event_tx.subscribe(),
        }
    }

    /// 接続イベントを送信（内部用）
    pub(crate) fn emit_connection_event(&self, event: ConnectionEvent) {
        let _ = self.connection_event_tx.send(event);
    }

    /// チャネルハンドラーを取得
    pub async fn get_channel_handler(&self, name: &str) -> Option<ChannelHandler> {
        let handlers = self.channel_handlers.read().await;
        handlers.get(name).cloned()
    }

    /// 接続の待ち受け開始（self を消費してブロック）
    ///
    /// サーバーを起動し、接続を受け付ける。終了するまでブロックする。
    /// 非ブロッキングで起動する場合は `spawn_listen()` を使用する。
    ///
    /// **注意**: self を消費するため、`subscribe_connection_events()` は
    /// このメソッドの呼び出し前に行う必要がある。
    pub async fn listen(self, addr: &str) -> Result<(), NetworkError> {
        use super::quic::QuicServer;

        let protocol_server = Arc::new(self);
        protocol_server.running.store(true, Ordering::SeqCst);

        let mut quic_server = QuicServer::new(Arc::clone(&protocol_server));
        quic_server
            .bind(addr)
            .await
            .map_err(|e| NetworkError::Quic(e.to_string()))?;

        tracing::info!("Unison Protocol server listening on {} via QUIC", addr);

        let result = quic_server
            .start()
            .await
            .map_err(|e| NetworkError::Quic(e.to_string()));

        protocol_server.running.store(false, Ordering::SeqCst);
        result
    }

    /// バックグラウンドでサーバーを起動し、ServerHandle を返す
    ///
    /// `ServerHandle::shutdown()` でグレースフルに停止できる。
    ///
    /// **注意**: self を消費するため、`subscribe_connection_events()` は
    /// このメソッドの呼び出し前に行う必要がある。
    pub async fn spawn_listen(self, addr: &str) -> Result<ServerHandle, NetworkError> {
        Arc::new(self).spawn_listen_shared(addr).await
    }

    /// `spawn_listen` の `Arc<Self>` 版 (v0.10.0 で追加、 broadcast 用 path)
    ///
    /// `spawn_listen` が `self` を consume するため、 broadcast 等で server へ
    /// outside reference を保ちたい caller は本 method を使う。 caller が `Arc::clone`
    /// を保持してそれを通して `server.broadcast(...)` を呼べる。
    pub async fn spawn_listen_shared(
        self: Arc<Self>,
        addr: &str,
    ) -> Result<ServerHandle, NetworkError> {
        use super::quic::QuicServer;

        let protocol_server = self;

        let mut quic_server = QuicServer::new(Arc::clone(&protocol_server));
        quic_server
            .bind(addr)
            .await
            .map_err(|e| NetworkError::Quic(e.to_string()))?;

        let local_addr = quic_server
            .local_addr()
            .ok_or_else(|| NetworkError::Quic("Server not bound".to_string()))?;

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        protocol_server.running.store(true, Ordering::SeqCst);

        tracing::info!("Unison Protocol server spawned on {} via QUIC", local_addr);

        let server_clone = Arc::clone(&protocol_server);
        let join_handle = tokio::spawn(async move {
            let result = quic_server
                .start_with_shutdown(shutdown_rx)
                .await
                .map_err(|e| NetworkError::Quic(e.to_string()));

            server_clone.running.store(false, Ordering::SeqCst);

            result
        });

        Ok(ServerHandle {
            join_handle,
            shutdown_tx: Some(shutdown_tx),
            local_addr,
        })
    }
}

impl Default for ProtocolServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_creation() {
        let server = ProtocolServer::new();
        assert!(!server.is_running());
    }

    #[tokio::test]
    async fn test_server_lifecycle() {
        let server = ProtocolServer::new();

        // チャネルハンドラーを登録
        server
            .register_channel("ping", |_ctx, _stream| async { Ok(()) })
            .await;

        // チャネルハンドラーが取得できること
        let handler = server.get_channel_handler("ping").await;
        assert!(handler.is_some());
    }

    #[tokio::test]
    async fn test_recv_skip_lagged_normal() {
        // 通常のイベント受信が正しく動作すること
        let server = ProtocolServer::new();
        let mut rx = server.subscribe_connection_events();

        let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        server.emit_connection_event(ConnectionEvent::Disconnected { remote_addr: addr });

        let event = rx.recv_skip_lagged().await.unwrap();
        match event {
            ConnectionEvent::Disconnected { remote_addr } => {
                assert_eq!(remote_addr, addr);
            }
            _ => panic!("Expected Disconnected event"),
        }
    }

    #[tokio::test]
    async fn test_recv_skip_lagged_skips_lagged() {
        // capacity を超えるイベントを送信し、recv_skip_lagged が Lagged をスキップすること
        let (tx, _) = tokio::sync::broadcast::channel::<ConnectionEvent>(2);

        let mut rx = ConnectionEventReceiver {
            inner: tx.subscribe(),
        };

        let addr: SocketAddr = "127.0.0.1:8001".parse().unwrap();

        // capacity(2) を超える 4 件を送信 → subscriber は Lagged になる
        for _ in 0..4 {
            let _ = tx.send(ConnectionEvent::Disconnected { remote_addr: addr });
        }

        // recv_skip_lagged は Lagged をスキップして最新のイベントを返す
        let event = rx.recv_skip_lagged().await.unwrap();
        match event {
            ConnectionEvent::Disconnected { remote_addr } => {
                assert_eq!(remote_addr, addr);
            }
            _ => panic!("Expected Disconnected event"),
        }
    }

    #[tokio::test]
    async fn test_recv_skip_lagged_returns_closed() {
        // sender がドロップされた場合に Closed が返ること
        let (tx, _) = tokio::sync::broadcast::channel::<ConnectionEvent>(2);

        let mut rx = ConnectionEventReceiver {
            inner: tx.subscribe(),
        };

        // sender をドロップ
        drop(tx);

        let result = rx.recv_skip_lagged().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            tokio::sync::broadcast::error::RecvError::Closed => {}
            other => panic!("Expected Closed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_recv_delegates_directly() {
        // recv() は Lagged エラーをそのまま返すこと（スキップしない）
        let (tx, _) = tokio::sync::broadcast::channel::<ConnectionEvent>(2);

        let mut rx = ConnectionEventReceiver {
            inner: tx.subscribe(),
        };

        let addr: SocketAddr = "127.0.0.1:8002".parse().unwrap();

        // capacity を超える送信
        for _ in 0..4 {
            let _ = tx.send(ConnectionEvent::Disconnected { remote_addr: addr });
        }

        // recv() は Lagged をそのまま返す
        let result = rx.recv().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            tokio::sync::broadcast::error::RecvError::Lagged(n) => {
                assert!(n > 0, "Lagged count should be > 0");
            }
            other => panic!("Expected Lagged, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_inner_access() {
        // inner() 経由で broadcast::Receiver に直接アクセスして recv() できること
        let server = ProtocolServer::new();
        let mut rx = server.subscribe_connection_events();

        let addr: SocketAddr = "127.0.0.1:7001".parse().unwrap();
        server.emit_connection_event(ConnectionEvent::Disconnected { remote_addr: addr });

        // inner() で内部の broadcast::Receiver を取得し、直接 recv() する
        let event = rx.inner().recv().await.unwrap();
        match event {
            ConnectionEvent::Disconnected { remote_addr } => {
                assert_eq!(remote_addr, addr);
            }
            _ => panic!("Expected Disconnected event"),
        }
    }

    #[tokio::test]
    async fn test_lagged_skip_then_new_event() {
        // Lagged → スキップ → 新規イベント正常受信の連続フローテスト
        let (tx, _) = tokio::sync::broadcast::channel::<ConnectionEvent>(2);

        let mut rx = ConnectionEventReceiver {
            inner: tx.subscribe(),
        };

        let addr: SocketAddr = "127.0.0.1:7002".parse().unwrap();

        // capacity(2) を超える 4 件を送信 → subscriber は Lagged になる
        for _ in 0..4 {
            let _ = tx.send(ConnectionEvent::Disconnected { remote_addr: addr });
        }

        // recv_skip_lagged で Lagged をスキップして最新イベントを受信
        let event = rx.recv_skip_lagged().await.unwrap();
        match &event {
            ConnectionEvent::Disconnected { remote_addr } => {
                assert_eq!(*remote_addr, addr);
            }
            _ => panic!("Expected Disconnected event after lagged skip"),
        }

        // バッファに残っているイベントを消費
        // capacity 2 で 4 件送信後にスキップ → 残り 1 件が取得可能な場合がある
        while rx.inner().try_recv().is_ok() {}

        // Lagged 回復後に新しいイベントを送信
        let new_addr: SocketAddr = "127.0.0.1:7003".parse().unwrap();
        let _ = tx.send(ConnectionEvent::Connected {
            remote_addr: new_addr,
            context: Arc::new(super::super::context::ConnectionContext::new()),
        });

        // 新しいイベントが recv_skip_lagged で正常に受信できること
        let new_event = rx.recv_skip_lagged().await.unwrap();
        match new_event {
            ConnectionEvent::Connected { remote_addr, .. } => {
                assert_eq!(remote_addr, new_addr);
            }
            _ => panic!("Expected Connected event for new address"),
        }
    }
}
