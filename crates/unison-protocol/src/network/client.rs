use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::codec::{Codec, JsonCodec};

use super::channel::UnisonChannel;
use super::context::ConnectionContext;
use super::datagram_channel::DatagramChannel;
use super::datagram_dispatcher::DatagramDispatcher;
use super::identity::ServerIdentity;
use super::quic::{FRAME_TYPE_PROTOCOL, QuicClient, UnisonStream, write_typed_frame};
use super::{MessageType, NetworkError, ProtocolMessage};

/// QUIC protocol client implementation
pub struct ProtocolClient {
    transport: Arc<QuicClient>,
    /// 接続コンテキスト（Identity情報・チャネル状態）
    context: Arc<ConnectionContext>,
    /// Datagram dispatcher (= lazy spawn on first `open_datagram_channel`、 v0.10.0 で追加)
    datagram_dispatcher: Mutex<Option<Arc<DatagramDispatcher>>>,
}

impl ProtocolClient {
    pub fn new(transport: QuicClient) -> Self {
        Self {
            transport: Arc::new(transport),
            context: Arc::new(ConnectionContext::new()),
            datagram_dispatcher: Mutex::new(None),
        }
    }

    /// Create a new client with QUIC transport
    pub fn new_default() -> Result<Self> {
        let transport = QuicClient::new()?;
        Ok(Self {
            transport: Arc::new(transport),
            context: Arc::new(ConnectionContext::new()),
            datagram_dispatcher: Mutex::new(None),
        })
    }

    /// 接続コンテキストを取得
    pub fn context(&self) -> &Arc<ConnectionContext> {
        &self.context
    }

    /// サーバーから受信したIdentity情報を取得
    pub async fn server_identity(&self) -> Option<ServerIdentity> {
        self.context.identity().await
    }

    /// チャネルを開く（UnisonChannel を返す）
    ///
    /// `__channel:{name}` メソッドで新しいQUICストリームを開き、
    /// `UnisonChannel` でラップして返す。
    pub async fn open_channel(&self, channel_name: &str) -> Result<UnisonChannel, NetworkError> {
        let connection_guard = self.transport.connection().read().await;
        let connection = connection_guard
            .as_ref()
            .ok_or(NetworkError::NotConnected)?;

        // 新しい双方向ストリームを開く
        let (mut send_stream, recv_stream) = connection
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

        // UnisonStreamを作成してUnisonChannelでラップ
        let conn_arc = Arc::new(connection.clone());
        let stream = UnisonStream::from_streams(
            request_id,
            format!("__channel:{}", channel_name),
            conn_arc,
            send_stream,
            recv_stream,
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
        let connection_arc = Arc::new(connection.clone());
        drop(connection_guard);

        // Datagram dispatcher を lazy spawn
        let dispatcher = {
            let mut guard = self.datagram_dispatcher.lock().await;
            if guard.is_none() {
                *guard = Some(Arc::new(DatagramDispatcher::spawn(Arc::clone(&connection_arc))));
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

    /// サーバーからの切断
    pub async fn disconnect(&self) -> Result<(), NetworkError> {
        self.transport
            .disconnect()
            .await
            .map_err(|e| NetworkError::Connection(e.to_string()))
    }

    /// クライアント接続状態の確認
    pub async fn is_connected(&self) -> bool {
        self.transport.is_connected().await
    }
}

use super::generate_request_id;
