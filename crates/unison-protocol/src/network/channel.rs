//! Channel: Unified Channel 通信プリミティブ
//!
//! 各ChannelはQUICストリームにマッピングされ、
//! 独立したHoL Blocking境界を形成する。
//!
//! `UnisonChannel<C: Codec>` — 統合チャネル型（request/response + event push + raw bytes）
//!
//! Codec 型パラメータにより、JSON / protobuf 等のフォーマットを差し替え可能。

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::codec::{Codec, Decodable, Encodable, JsonCodec};

use super::quic::{TypedFrame, UnisonStream};
use super::{MessageType, NetworkError, ProtocolMessage};

/// デフォルトの request タイムアウト（30秒）
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 統合チャネル型 — Request/Response、Event、Raw bytes をサポート
///
/// 内部に recv ループを持ち、受信フレームを type tag で振り分ける:
/// - Protocol frame (0x00):
///   - `Response` → pending の oneshot に送る
///   - `Event` / その他 → event_rx に流す
/// - Raw frame (0x01) → raw_rx に流す
pub struct UnisonChannel<C: Codec = JsonCodec> {
    /// QUIC ストリームへの参照（送信用）
    stream: Arc<UnisonStream>,
    /// 応答待ちの Request を管理（message_id → oneshot::Sender）
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<ProtocolMessage>>>>,
    /// Event 受信キュー
    event_rx: Mutex<mpsc::Receiver<ProtocolMessage>>,
    /// Raw bytes 受信キュー
    raw_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
    /// メッセージ ID カウンター
    next_id: AtomicU64,
    /// バックグラウンド受信タスク
    recv_task: Mutex<Option<JoinHandle<()>>>,
    /// request() のタイムアウト
    request_timeout: Duration,
    /// Codec 型マーカー
    _codec: PhantomData<C>,
}

impl<C: Codec> UnisonChannel<C> {
    /// UnisonStream から UnisonChannel を構築し、recv ループを起動する
    pub fn new(stream: UnisonStream) -> Self {
        let stream = Arc::new(stream);
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<ProtocolMessage>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (event_tx, event_rx) = mpsc::channel(256);
        let (raw_tx, raw_rx) = mpsc::channel(256);

        // recv ループ — recv_typed_frame() で type tag ベースの振り分け
        let recv_stream = Arc::clone(&stream);
        let recv_pending = Arc::clone(&pending);
        let recv_task = tokio::spawn(async move {
            loop {
                match recv_stream.recv_typed_frame().await {
                    Ok(TypedFrame::Protocol(msg)) => {
                        match msg.msg_type {
                            MessageType::Response => {
                                let mut map = recv_pending.lock().await;
                                if let Some(sender) = map.remove(&msg.id) {
                                    let _ = sender.send(msg);
                                }
                            }
                            MessageType::Error => {
                                let mut map = recv_pending.lock().await;
                                if let Some(sender) = map.remove(&msg.id) {
                                    let _ = sender.send(msg);
                                } else {
                                    drop(map);
                                    let _ = event_tx.send(msg).await;
                                }
                            }
                            _ => {
                                // Event, Request, その他 → event_rx に流す
                                let _ = event_tx.send(msg).await;
                            }
                        }
                    }
                    Ok(TypedFrame::Raw(data)) => {
                        let _ = raw_tx.send(data).await;
                    }
                    Err(_) => {
                        // 接続断 — 全 pending を Error で解決
                        let mut map = recv_pending.lock().await;
                        for (_, sender) in map.drain() {
                            if let Ok(err_msg) = ProtocolMessage::new_with_json(
                                0,
                                "error".to_string(),
                                MessageType::Error,
                                serde_json::json!({"error": "connection closed"}),
                            ) {
                                let _ = sender.send(err_msg);
                            }
                        }
                        break;
                    }
                }
            }
        });

        Self {
            stream,
            pending,
            event_rx: Mutex::new(event_rx),
            raw_rx: Mutex::new(raw_rx),
            next_id: AtomicU64::new(1),
            recv_task: Mutex::new(Some(recv_task)),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            _codec: PhantomData,
        }
    }

    /// request タイムアウトを設定（ビルダーパターン）
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// 型付き Request/Response パターン
    ///
    /// メッセージ ID を自動生成し、pending マップに登録。
    /// Response が返るまで await する。
    ///
    /// `Req` / `Resp` は Codec `C` に対して Encodable / Decodable であれば何でも使える:
    /// - `UnisonChannel<JsonCodec>`: `serde::Serialize` / `DeserializeOwned` な型
    /// - `UnisonChannel<ProtoCodec>`: `buffa::Message` な型
    pub async fn request<Req, Resp>(&self, method: &str, req: &Req) -> Result<Resp, NetworkError>
    where
        Req: Encodable<C>,
        Resp: Decodable<C>,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        // pending に登録
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Request メッセージを Codec でエンコードしてフレームとして送信
        let payload = req.encode().map_err(NetworkError::Codec)?;
        let msg =
            ProtocolMessage::new_encoded(id, method.to_string(), MessageType::Request, payload);
        self.stream.send_frame(&msg).await?;

        // Response を待つ（タイムアウト付き）
        let response = match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err(NetworkError::Protocol(
                    "Request cancelled: channel closed".to_string(),
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(NetworkError::Timeout);
            }
        };

        match response.msg_type {
            MessageType::Error => {
                // エラーレスポンスは常に JSON (プロトコル内部)
                let payload = response.payload_as_value()?;
                Err(NetworkError::Protocol(format!(
                    "Request error: {}",
                    payload
                )))
            }
            _ => response.decode_payload::<Resp, C>(),
        }
    }

    /// 型付き Event 送信（応答不要）
    pub async fn send_event<T: Encodable<C>>(
        &self,
        method: &str,
        payload: &T,
    ) -> Result<(), NetworkError> {
        let bytes = payload.encode().map_err(NetworkError::Codec)?;
        let msg = ProtocolMessage::new_encoded(0, method.to_string(), MessageType::Event, bytes);
        self.stream.send_frame(&msg).await
    }

    /// 型付き Response 送信（サーバー側パターン）
    ///
    /// `recv()` で受け取った Request の `id` を指定して
    /// Response メッセージを返す。
    pub async fn send_response<T: Encodable<C>>(
        &self,
        request_id: u64,
        method: &str,
        payload: &T,
    ) -> Result<(), NetworkError> {
        let bytes = payload.encode().map_err(NetworkError::Codec)?;
        let msg = ProtocolMessage::new_encoded(
            request_id,
            method.to_string(),
            MessageType::Response,
            bytes,
        );
        self.stream.send_frame(&msg).await
    }

    /// Raw bytes 送信（rkyv/zstd をバイパス、最小オーバーヘッド）
    ///
    /// オーディオストリーミング等のバイナリデータに使用。
    pub async fn send_raw(&self, data: &[u8]) -> Result<(), NetworkError> {
        self.stream.send_raw_frame(data).await
    }

    /// Raw bytes 受信
    ///
    /// recv ループが type tag 0x01 のフレームを受信すると raw_rx に流す。
    pub async fn recv_raw(&self) -> Result<Vec<u8>, NetworkError> {
        let mut rx = self.raw_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| NetworkError::Protocol("Raw channel closed".to_string()))
    }

    /// Event 受信（サーバーからのプッシュ、または非 Response メッセージ）
    pub async fn recv(&self) -> Result<ProtocolMessage, NetworkError> {
        let mut rx = self.event_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| NetworkError::Protocol("Channel closed".to_string()))
    }

    /// チャネルを閉じる
    pub async fn close(&self) -> Result<(), NetworkError> {
        // recv タスクを中止
        if let Some(task) = self.recv_task.lock().await.take() {
            task.abort();
        }
        // ストリームを閉じる
        self.stream.close_stream().await
    }
}
