use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::broadcast;

use crate::codec::{Codec, JsonCodec};

use super::channel::UnisonChannel;
use super::context::ConnectionContext;
use super::datagram_channel::DatagramChannel;
use super::datagram_dispatcher::DatagramDispatcher;
use super::identity::ServerIdentity;
use super::quic::{
    CHANNEL_ACK_METHOD, FRAME_TYPE_PROTOCOL, QuicClient, UnisonStream, read_typed_frame,
    write_typed_frame,
};
use super::{MessageType, NetworkError, ProtocolMessage};

/// Client side connection event (v0.10.0 で追加、 [`ProtocolServer::ConnectionEvent`] と parallel)
///
/// `Connected` は `connect()` 成功時に 1 回 fire される。 `Disconnected` は connection が
/// drop された時 (= 明示 disconnect / server 側 close / 通信 error 何れか) に **1 回以上**
/// fire される (= 明示 `disconnect()` 時は explicit fire と drop detection task fire の
/// 2 件が重なる場合がある、 詳細は [`ProtocolClient::disconnect`] を参照)。 subscriber は
/// **冪等性を持つ**形で扱う責務がある (= 「Disconnected を連続で受けても 1 回の disconnect」
/// として扱う、 reason 文字列を見て filter する 等)。
///
/// Library は auto-reconnect しない (= caller がこの event を見て自身のポリシーで
/// 再接続を実行する責務を持つ)。
#[derive(Debug, Clone)]
pub enum ClientConnectionEvent {
    /// Server へ接続確立 (= `connect()` 成功時に fire)
    Connected {
        /// 接続先 server の SocketAddr
        remote_addr: SocketAddr,
    },
    /// Server との接続切断 (= 明示 `disconnect()` / 受動 drop どちらでも fire)
    Disconnected {
        /// 切断理由 (= caller が再接続判断に使う、 free text)
        reason: String,
    },
}

/// [`ProtocolClient::subscribe_connection_events`] が返す event receiver
///
/// `tokio::sync::broadcast::Receiver<ClientConnectionEvent>` のラッパー、
/// server 側 [`super::server::ConnectionEventReceiver`] と parallel な API surface。
pub struct ClientConnectionEventReceiver {
    inner: broadcast::Receiver<ClientConnectionEvent>,
}

impl ClientConnectionEventReceiver {
    /// 内部の broadcast::Receiver を直接参照する
    pub fn inner(&mut self) -> &mut broadcast::Receiver<ClientConnectionEvent> {
        &mut self.inner
    }

    /// 次の connection event を受信する (= `recv` raw、 Lagged は caller が判断)
    pub async fn recv(&mut self) -> Result<ClientConnectionEvent, broadcast::error::RecvError> {
        self.inner.recv().await
    }

    /// 次の connection event を受信、 `Lagged` (= subscriber 消費遅れで buffer 一巡)
    /// は透過的に skip して最新から再開
    pub async fn recv_skip_lagged(
        &mut self,
    ) -> Result<ClientConnectionEvent, broadcast::error::RecvError> {
        loop {
            match self.inner.recv().await {
                Ok(ev) => return Ok(ev),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(e @ broadcast::error::RecvError::Closed) => return Err(e),
            }
        }
    }
}

/// QUIC protocol client implementation
pub struct ProtocolClient {
    transport: Arc<QuicClient>,
    /// 接続コンテキスト（Identity情報・チャネル状態）
    context: Arc<ConnectionContext>,
    /// Datagram dispatcher (= lazy spawn on first `open_datagram_channel`、 v0.10.0 で追加)
    datagram_dispatcher: Mutex<Option<Arc<DatagramDispatcher>>>,
    /// Connection event broadcast (v0.10.0 で追加、 server `connection_event_tx` と parallel)
    ///
    /// capacity 16: 1 client の lifecycle event (= Connected / Disconnected) は
    /// 再接続 burst でも 10/秒 を超えない想定、 16 件 buffer で十分。
    connection_event_tx: broadcast::Sender<ClientConnectionEvent>,
}

impl ProtocolClient {
    pub fn new(transport: QuicClient) -> Self {
        let (event_tx, _) = broadcast::channel(16);
        Self {
            transport: Arc::new(transport),
            context: Arc::new(ConnectionContext::new()),
            datagram_dispatcher: Mutex::new(None),
            connection_event_tx: event_tx,
        }
    }

    /// Create a new client with QUIC transport.
    ///
    /// **注意**: この constructor は [`QuicClient::new`] を経由し、証明書検証を
    /// 行わない insecure な client を構築する (構築時に `tracing::warn!` が出る)。
    /// production では [`QuicClient::builder`] + [`ProtocolClient::new`] で
    /// 明示的に [`TrustAnchors`] を指定すること。
    ///
    /// [`TrustAnchors`]: crate::network::trust::TrustAnchors
    pub fn new_default() -> Result<Self> {
        let transport = QuicClient::new()?;
        let (event_tx, _) = broadcast::channel(16);
        Ok(Self {
            transport: Arc::new(transport),
            context: Arc::new(ConnectionContext::new()),
            datagram_dispatcher: Mutex::new(None),
            connection_event_tx: event_tx,
        })
    }

    /// Connection lifecycle event (= Connected / Disconnected) を subscribe する
    /// (v0.10.0 で追加)
    ///
    /// Server 側 [`super::server::ProtocolServer::subscribe_connection_events`] と
    /// parallel な API。 caller は subscribe 後に [`ClientConnectionEventReceiver::recv`]
    /// で event を読む。 複数の caller が同時に subscribe 可能 (= broadcast)。
    ///
    /// 注: library は自動 reconnect しない。 caller が `Disconnected` を受け取ったら
    /// 自身のポリシーで `client.connect(url)` を再呼び出しする責務を持つ。
    pub fn subscribe_connection_events(&self) -> ClientConnectionEventReceiver {
        ClientConnectionEventReceiver {
            inner: self.connection_event_tx.subscribe(),
        }
    }

    /// 接続コンテキストを取得
    pub fn context(&self) -> &Arc<ConnectionContext> {
        &self.context
    }

    /// サーバーから受信したIdentity情報を取得
    pub async fn server_identity(&self) -> Option<ServerIdentity> {
        self.context.identity().await
    }

    /// server-initiated channel (= `from="server"`) の handler を登録する。
    ///
    /// server が [`ConnectionContext::open_server_stream`](super::context::ConnectionContext::open_server_stream)
    /// で開いた server→client の reliable push stream を、宣言 frame の method (= `channel`) で
    /// この handler に routing する。handler は渡された raw [`UnisonStream`] を **直読** して
    /// payload を取りこぼし無く受ける（recv ループ／中継 mpsc を挟まない = QUIC backpressure）。
    ///
    /// `connect` 前に登録しておくこと。同 `channel` で再登録すると古い handler を replace する。
    pub async fn register_server_channel<F, Fut>(&self, channel: &str, handler: F)
    where
        F: Fn(UnisonStream) -> Fut + Send + Sync + 'static,
        Fut: futures_util::Future<Output = Result<(), NetworkError>> + Send + 'static,
    {
        self.transport
            .register_server_channel(channel, handler)
            .await;
    }

    /// チャネルを開く（UnisonChannel を返す）
    ///
    /// `__channel:{name}` メソッドで新しいQUICストリームを開き、 サーバーが返す
    /// `open_ack` (= Phase 6c) を待ってから `UnisonChannel` でラップして返す。
    ///
    /// サーバーが未登録 channel に対し nack (= `__channel_ack` の Error frame) を
    /// 返した場合は [`NetworkError::Protocol`] で reject する (= channel-not-found)。
    /// `open_ack` を待つことで、 fire-and-forget だった旧挙動の「accept されたか
    /// 分からない」問題を解消する。
    pub async fn open_channel(&self, channel_name: &str) -> Result<UnisonChannel, NetworkError> {
        let connection_guard = self.transport.connection().read().await;
        let connection = connection_guard
            .as_ref()
            .ok_or(NetworkError::NotConnected)?;

        // 新しい双方向ストリームを開く
        let (mut send_stream, mut recv_stream) = connection
            .open_bi()
            .await
            .map_err(|e| NetworkError::Quic(format!("Failed to open channel stream: {}", e)))?;

        // チャネル識別メッセージを送信（length-prefixed）
        let method = format!("__channel:{}", channel_name);
        let request_id = generate_request_id();
        let message = ProtocolMessage::new_with_json(
            request_id,
            method,
            MessageType::Request,
            serde_json::json!({}),
        )?;

        let frame = message.into_frame().map_err(|e| {
            NetworkError::Protocol(format!("Failed to create channel frame: {}", e))
        })?;
        let frame_bytes = frame.to_bytes();
        write_typed_frame(&mut send_stream, FRAME_TYPE_PROTOCOL, &frame_bytes)
            .await
            .map_err(|e| NetworkError::Protocol(format!("Failed to send channel open: {}", e)))?;

        // Phase 6c: サーバーの open_ack を待つ。 server は handler を起動する前に
        // 同 stream へ `__channel_ack` frame を 1 本返す。 これを recv loop に渡る
        // 前にここで read することで、 accept されたかを確定させる。
        let ack = read_channel_ack(&mut recv_stream).await?;
        match ack.msg_type {
            MessageType::Response => {
                tracing::debug!("Channel '{}' open_ack received", channel_name);
            }
            MessageType::Error => {
                let payload = ack.payload_as_value().unwrap_or_default();
                return Err(NetworkError::Protocol(format!(
                    "Channel '{}' open rejected: {}",
                    channel_name, payload
                )));
            }
            other => {
                return Err(NetworkError::Protocol(format!(
                    "Channel '{}' open: unexpected ack msg_type {:?}",
                    channel_name, other
                )));
            }
        }

        // UnisonStreamを作成してUnisonChannelでラップ
        // quinn のストリームを transport 非依存の trait object へ box する。
        let conn_arc: Arc<dyn super::conn::UnisonConn> = Arc::new(connection.clone());
        let stream = UnisonStream::from_streams(
            request_id,
            format!("__channel:{}", channel_name),
            conn_arc,
            Box::new(send_stream),
            Box::new(recv_stream),
        );

        // コンテキストにチャネルを登録
        self.context
            .register_channel(super::context::ChannelHandle {
                channel_name: channel_name.to_string(),
                stream_id: request_id,
                direction: super::identity::ChannelDirection::Bidirectional,
            })
            .await;

        Ok(UnisonChannel::new(stream))
    }

    /// Datagram channel を open (v0.10.0 で追加、 default codec = JsonCodec)
    ///
    /// 同 connection で初回 call 時に `DatagramDispatcher` を lazy spawn、 以降は
    /// 既存 dispatcher を再利用する。 caller は `channel_id` (= KDL schema で割り当て
    /// た値) を明示で渡す責任を持つ (= codegen が `client.open_datagram_channel(name,
    /// channel_id)` の形で生成する)。
    ///
    /// 別 codec を使いたい場合は [`Self::open_datagram_channel_with`] を使用。
    pub async fn open_datagram_channel(
        &self,
        channel_name: &str,
        channel_id: u64,
    ) -> Result<DatagramChannel<JsonCodec>, NetworkError> {
        self.open_datagram_channel_with::<JsonCodec>(channel_name, channel_id)
            .await
    }

    /// Datagram channel を open する codec generic 版 (v0.10.0)
    ///
    /// [`Self::open_datagram_channel`] と同じだが任意 codec C を指定可能。
    pub async fn open_datagram_channel_with<C: Codec>(
        &self,
        channel_name: &str,
        channel_id: u64,
    ) -> Result<DatagramChannel<C>, NetworkError> {
        // 接続中の connection を取得
        let connection_guard = self.transport.connection().read().await;
        let connection = connection_guard
            .as_ref()
            .ok_or(NetworkError::NotConnected)?;
        let connection_arc: Arc<dyn super::conn::UnisonConn> = Arc::new(connection.clone());
        drop(connection_guard);

        // Datagram dispatcher を lazy spawn
        let dispatcher = {
            let mut guard = self.datagram_dispatcher.lock().await;
            if guard.is_none() {
                *guard = Some(Arc::new(DatagramDispatcher::spawn(Arc::clone(
                    &connection_arc,
                ))));
            }
            Arc::clone(guard.as_ref().unwrap())
        };

        // channel_id を dispatcher に登録、 receiver を取得
        // buffer 256: position 等 60Hz × 数秒分のバースト吸収を想定
        let recv_rx = dispatcher.register(channel_id, 256).await;

        Ok(DatagramChannel::<C>::new(
            connection_arc,
            channel_id,
            channel_name.to_string(),
            recv_rx,
        ))
    }

    /// 接続後にサーバーからIdentityを受信する
    ///
    /// Identity 専用の oneshot チャネルから受信するため、
    /// 他のメッセージが先に到着しても影響を受けない。
    async fn receive_identity(&self) -> Result<ServerIdentity, NetworkError> {
        let response = self
            .transport
            .receive_identity(std::time::Duration::from_secs(10))
            .await
            .map_err(|e| NetworkError::Protocol(format!("Failed to receive identity: {}", e)))?;

        // oneshot に送られるのは常に __identity のみ（client_accept_bi_loop で振り分け済み）
        debug_assert_eq!(
            response.method, "__identity",
            "oneshot routing invariant violated"
        );

        let identity = ServerIdentity::from_protocol_message(&response)
            .map_err(|e| NetworkError::Protocol(format!("Failed to parse identity: {}", e)))?;
        self.context.set_identity(identity.clone()).await;
        Ok(identity)
    }

    /// Unisonサーバーへの接続（Identity Handshake 含む）
    pub async fn connect(&self, url: &str) -> Result<(), NetworkError> {
        self.transport
            .connect(url)
            .await
            .map_err(|e| NetworkError::Connection(e.to_string()))?;

        // reconnect 対策: 古い connection に bind された datagram dispatcher を破棄する。
        // dispatcher は spawn 時の connection を握ったままなので、 これを clear しないと
        // reconnect 後の open_datagram_channel が死んだ旧 connection の dispatcher を
        // 再利用し、 datagram が一切流れなくなる (= silently dead)。 次回 open 時に
        // 新 connection に対して lazy 再 spawn される。
        *self.datagram_dispatcher.lock().await = None;

        // v0.10.0 Step 2: Connected event を fire (= subscribe している caller に通知)
        // remote_addr を connection から取得 (= ない場合は空 SocketAddr で fallback)
        let remote_addr = {
            let guard = self.transport.connection().read().await;
            guard
                .as_ref()
                .map(|c| c.remote_address())
                .unwrap_or_else(|| "[::]:0".parse().expect("fallback addr parse"))
        };
        let _ = self
            .connection_event_tx
            .send(ClientConnectionEvent::Connected { remote_addr });

        // v0.10.0 Step 2: drop detection task を spawn
        // QUIC connection の `closed()` future が resolve したら自動的に Disconnected を fire
        // (= server 側 close / network error / 明示 disconnect 何れでも発火)
        self.spawn_drop_detection_task(remote_addr).await;

        // Identity Handshake: サーバーからIdentityを受信
        match self.receive_identity().await {
            Ok(identity) => {
                tracing::info!(
                    "Received server identity: {} v{}",
                    identity.name,
                    identity.version
                );
            }
            Err(e) => {
                tracing::warn!("Failed to receive identity (non-fatal): {}", e);
            }
        }

        Ok(())
    }

    /// Unisonサーバーへ接続し、 接続直後に credential を1回提示して認証する
    /// (= connection-level authN)。
    ///
    /// [`connect`](Self::connect) → `unison.auth` channel を open → `Authenticate`
    /// request で credential を送り、 server の verifier 結果を待つ。認証が拒否された
    /// (= `AuthResult { ok: false }`) 場合は [`NetworkError::Protocol`] で reject する。
    ///
    /// 認証成功後、 server 側はこの connection の principal を立てているので、 以降
    /// open する worlds/wire/datagram 等の channel は per-message gate を通過できる。
    /// **他 channel を open する前に本メソッドを呼ぶこと** (= 未認証だと principal=None で
    /// app handler に gate される)。
    ///
    /// server が `enable_auth` を呼んでいない場合は `unison.auth` が未登録のため open が
    /// reject される。認証不要の server には [`connect`](Self::connect) を使う。
    ///
    /// 設計: `design/connection-auth.md`
    pub async fn connect_with_credential(
        &self,
        url: &str,
        credential: &[u8],
    ) -> Result<(), NetworkError> {
        self.connect(url).await?;

        let channel = self.open_channel(super::auth::AUTH_CHANNEL_NAME).await?;
        let response: serde_json::Value = channel
            .request(
                super::auth::AUTHENTICATE_METHOD,
                &serde_json::json!({ "credential": credential }),
            )
            .await?;
        let result: super::auth::AuthResult = serde_json::from_value(response)?;
        let _ = channel.close().await;

        if !result.ok {
            return Err(NetworkError::Protocol(
                "authentication denied by server verifier".to_string(),
            ));
        }
        Ok(())
    }

    /// Connection の `closed()` future を await して Disconnected event を fire する task
    /// を spawn (v0.10.0 Step 2)
    ///
    /// connect() のたびに新 task が spawn される。 連続 reconnect で task が重複しても
    /// 古い connection は既に closed なので即終了する (= leak しない)。
    async fn spawn_drop_detection_task(&self, remote_addr: SocketAddr) {
        let event_tx = self.connection_event_tx.clone();
        let connection_handle = {
            let guard = self.transport.connection().read().await;
            guard.as_ref().cloned()
        };
        if let Some(connection) = connection_handle {
            tokio::spawn(async move {
                let close_reason = connection.closed().await;
                let _ = event_tx.send(ClientConnectionEvent::Disconnected {
                    reason: format!("connection closed: {}", close_reason),
                });
                tracing::debug!(
                    "Drop detection task fired Disconnected for {}: {}",
                    remote_addr,
                    close_reason
                );
            });
        }
    }

    /// サーバーからの切断
    pub async fn disconnect(&self) -> Result<(), NetworkError> {
        self.transport
            .disconnect()
            .await
            .map_err(|e| NetworkError::Connection(e.to_string()))?;
        // dispatcher は旧 connection に bind されている。 破棄して task を終わらせ、
        // 再接続時にクリーンに再 spawn できるようにする。
        *self.datagram_dispatcher.lock().await = None;
        // v0.10.0 Step 2: 明示 disconnect でも Disconnected event を fire (= subscribe
        // 側で「自分で disconnect した」 を別 path で識別したい場合は reason 文字列で判定)
        // 注: spawn_drop_detection_task の `closed().await` も同時に fire するため、
        // 同 disconnect で 2 件 event が流れる可能性がある。 Subscriber 側は冪等性を持つ
        // (= 「Disconnected を 2 回連続で受けても 1 回の disconnect」 として扱う) のが原則。
        let _ = self
            .connection_event_tx
            .send(ClientConnectionEvent::Disconnected {
                reason: "explicit disconnect by caller".to_string(),
            });
        Ok(())
    }

    /// クライアント接続状態の確認
    pub async fn is_connected(&self) -> bool {
        self.transport.is_connected().await
    }
}

use super::generate_request_id;

/// Channel open 後の `open_ack` (= `__channel_ack` frame) を recv ストリームから
/// 1 本読む (= Phase 6c)。
///
/// サーバーは handler 起動の前に同 stream へ ack を返す。 frame type が
/// PROTOCOL でない / method が `__channel_ack` でない場合はプロトコル違反として
/// `Err`。 stream が ack 到着前に終端した場合も `Err` (= no-accept signal)。
async fn read_channel_ack<R>(recv: &mut R) -> Result<ProtocolMessage, NetworkError>
where
    R: tokio::io::AsyncRead + Unpin + ?Sized,
{
    let (frame_type, frame_bytes) = read_typed_frame(recv)
        .await
        .map_err(|e| NetworkError::Protocol(format!("Failed to read open_ack frame: {}", e)))?;
    if frame_type != FRAME_TYPE_PROTOCOL {
        return Err(NetworkError::Protocol(format!(
            "open_ack: unexpected frame type 0x{:02x}",
            frame_type
        )));
    }
    let frame = super::ProtocolFrame::from_bytes(&frame_bytes)
        .map_err(|e| NetworkError::Protocol(format!("Failed to decode open_ack frame: {}", e)))?;
    let msg = ProtocolMessage::from_frame(&frame)
        .map_err(|e| NetworkError::Protocol(format!("Failed to parse open_ack: {}", e)))?;
    if msg.method != CHANNEL_ACK_METHOD {
        return Err(NetworkError::Protocol(format!(
            "open_ack: expected method '{}', got '{}'",
            CHANNEL_ACK_METHOD, msg.method
        )));
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `subscribe_connection_events` は subscriber を返し、 connect 前でも publish 済み
    /// event が無ければ recv が pending (= 別 task で連動して event を待つ pattern を担保)
    #[tokio::test]
    async fn subscribe_before_connect_receives_subsequent_events() {
        let client = ProtocolClient::new_default().unwrap();
        let mut rx = client.subscribe_connection_events();

        // 手動で event を publish (= 実 connect なしで broadcast 動作を確認)
        let _ = client
            .connection_event_tx
            .send(ClientConnectionEvent::Connected {
                remote_addr: "127.0.0.1:1234".parse().unwrap(),
            });

        let ev = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("recv timeout")
            .expect("recv error");
        match ev {
            ClientConnectionEvent::Connected { remote_addr } => {
                assert_eq!(remote_addr.port(), 1234);
            }
            other => panic!("expected Connected, got: {:?}", other),
        }
    }

    /// 複数 subscriber が同 event を独立に受信できる (= broadcast 性質)
    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let client = ProtocolClient::new_default().unwrap();
        let mut rx_a = client.subscribe_connection_events();
        let mut rx_b = client.subscribe_connection_events();

        let _ = client
            .connection_event_tx
            .send(ClientConnectionEvent::Disconnected {
                reason: "test".to_string(),
            });

        for rx in [&mut rx_a, &mut rx_b] {
            let ev = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(ev, ClientConnectionEvent::Disconnected { .. }));
        }
    }

    /// `recv_skip_lagged` が Lagged を透過、 最新 event を返す
    #[tokio::test]
    async fn recv_skip_lagged_skips_lag_returns_latest() {
        // capacity 16 を超えて publish → Lagged を生成
        let client = ProtocolClient::new_default().unwrap();
        let mut rx = client.subscribe_connection_events();

        for i in 0..20 {
            let _ = client
                .connection_event_tx
                .send(ClientConnectionEvent::Connected {
                    remote_addr: format!("127.0.0.1:{}", 1000 + i).parse().unwrap(),
                });
        }
        // recv_skip_lagged は Lagged を skip して buffer 内最古 を返す
        let ev = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv_skip_lagged())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ev, ClientConnectionEvent::Connected { .. }));
    }

    /// Receiver の inner() は &mut broadcast::Receiver を返す (= server 側 parallel API)
    #[tokio::test]
    async fn receiver_inner_exposes_broadcast_receiver() {
        let client = ProtocolClient::new_default().unwrap();
        let mut rx = client.subscribe_connection_events();
        // inner() の型が broadcast::Receiver であることを compile-check
        let _inner: &mut broadcast::Receiver<ClientConnectionEvent> = rx.inner();
    }
}
