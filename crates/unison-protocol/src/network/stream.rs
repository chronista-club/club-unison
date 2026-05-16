//! transport 非依存の双方向ストリーム実装。
//!
//! [`UnisonStream`] は内部に [`BoxUnisonSend`] / [`BoxUnisonRecv`] (= trait
//! object) を保持し、 raw QUIC と WebTransport のどちらのストリームでも同一の型で
//! 扱える。 これが `register_channel` ハンドラーに渡る面 (= handler-facing API)
//! なので、 型名・メソッドシグネチャは安定させている。

use anyhow::{Context, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::Mutex;
use tracing::info;

use super::conn::{BoxUnisonRecv, BoxUnisonSend, UnisonConn};
use super::frame::{FRAME_TYPE_PROTOCOL, FRAME_TYPE_RAW, read_typed_frame, write_typed_frame};
use super::{NetworkError, ProtocolFrame, ProtocolMessage};

/// Unison Stream — transport 非依存の双方向ストリーム実装。
///
/// 内部は [`BoxUnisonSend`] / [`BoxUnisonRecv`] (= trait object) を保持し、
/// raw QUIC と WebTransport のどちらのストリームでも同一の型で扱える。 これが
/// `register_channel` ハンドラーに渡る面 (= handler-facing API) なので、 型名・
/// メソッドシグネチャは安定させている。
pub struct UnisonStream {
    stream_id: u64,
    method: String,
    #[allow(dead_code)]
    connection: Arc<dyn UnisonConn>,
    send_stream: Arc<Mutex<Option<BoxUnisonSend>>>,
    recv_stream: Arc<Mutex<Option<BoxUnisonRecv>>>,
    is_active: Arc<AtomicBool>,
}

impl UnisonStream {
    pub async fn new(
        method: String,
        connection: Arc<dyn UnisonConn>,
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
        connection: Arc<dyn UnisonConn>,
        send_stream: BoxUnisonSend,
        recv_stream: BoxUnisonRecv,
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
    /// buffa/zstd をバイパスし、length-prefix + type tag + raw payload のみ。
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
            send_stream.finish().await?;
        }

        if let Some(mut recv_stream) = self.recv_stream.lock().await.take() {
            recv_stream.stop()?;
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
