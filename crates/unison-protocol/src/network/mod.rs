use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::codec::{CodecError, Decodable, Encodable, JsonCodec};
use crate::packet::{RkyvPayload, SerializationError, UnisonPacket};

pub mod cert;
pub mod channel;
pub mod client;
pub mod context;
pub mod identity;
pub mod mesh;
pub mod quic;
pub mod server;
pub mod trust;

pub use cert::CertSource;
pub use channel::UnisonChannel;
pub use client::ProtocolClient;
pub use mesh::InternalMeshKeypair;
pub use quic::{QuicClient, QuicServer, TypedFrame, UnisonStream};
pub use server::{ConnectionEvent, ConnectionEventReceiver, ProtocolServer, ServerHandle};
pub use trust::TrustAnchors;

/// グローバルなリクエストID生成（モジュール間で一意）
pub(crate) fn generate_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Unison Protocolのネットワークエラー
#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Codec error: {0}")]
    Codec(#[from] CodecError),
    #[error("Frame serialization error: {0}")]
    FrameSerialization(#[from] SerializationError),
    #[error("QUIC error: {0}")]
    Quic(String),
    #[error("Timeout error")]
    Timeout,
    #[error("Handler not found for method: {method}")]
    HandlerNotFound { method: String },
    #[error("Not connected")]
    NotConnected,
    #[error("Unsupported transport: {0}")]
    UnsupportedTransport(String),
}

impl NetworkError {
    /// この error が **正常な channel 終端** (= sender side 完了で drop) を表すか判定する。
    ///
    /// `UnisonChannel::recv()` / `recv_raw()` / `request()` は内部の sender / oneshot が
    /// drop された時に 3 種類の Protocol error を生成する:
    ///
    /// - `"Channel closed"` — `recv()` で mpsc receiver が None を返した
    /// - `"Raw channel closed"` — `recv_raw()` で raw mpsc receiver が None を返した
    /// - `"Request cancelled: channel closed"` — `request()` 中に oneshot sender が drop した
    ///
    /// これらは sender 側が request/response 完了後に正常 close した end-of-stream であり、
    /// 真の error ではない。 caller (= e.g. QUIC server の channel handler dispatcher) は
    /// log level を ERROR ではなく debug / info に degrade することで noise を抑えられる。
    ///
    /// 文字列マッチで判定しているため、将来追加されるパターンも忘れずにここを更新すること。
    /// (長期的には `NetworkError::ChannelEof` のような enum variant 化で型安全にすべき)
    pub fn is_normal_close(&self) -> bool {
        matches!(
            self,
            NetworkError::Protocol(msg)
                if msg == "Channel closed"
                || msg == "Raw channel closed"
                || msg == "Request cancelled: channel closed"
        )
    }
}

/// プロトコルメッセージラッパー
#[derive(Debug, Clone, Serialize, Deserialize, Archive, RkyvSerialize, RkyvDeserialize)]
#[archive(check_bytes)]
pub struct ProtocolMessage {
    pub id: u64,
    pub method: String,
    #[serde(rename = "type")]
    pub msg_type: MessageType,
    pub payload: Vec<u8>, // Codec がエンコードしたバイト列
}

/// フレームでラップされたプロトコルメッセージの型エイリアス
pub type ProtocolFrame = UnisonPacket<RkyvPayload<ProtocolMessage>>;

impl ProtocolMessage {
    /// ProtocolMessageをフレームに変換
    pub fn into_frame(self) -> Result<ProtocolFrame, SerializationError> {
        let payload = RkyvPayload::new(self);
        UnisonPacket::new(payload)
    }

    /// フレームからProtocolMessageを復元
    pub fn from_frame(frame: &ProtocolFrame) -> Result<Self, SerializationError> {
        let payload = frame.payload()?;
        Ok(payload.data.clone())
    }

    /// エンコード済みバイト列から ProtocolMessage を直接作成
    pub fn new_encoded(
        id: u64,
        method: String,
        msg_type: MessageType,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            id,
            method,
            msg_type,
            payload,
        }
    }

    /// JSON でエンコードして ProtocolMessage を作成
    pub fn new_with_json(
        id: u64,
        method: String,
        msg_type: MessageType,
        payload: serde_json::Value,
    ) -> Result<Self, NetworkError> {
        let bytes =
            Encodable::<JsonCodec>::encode(&payload).map_err(NetworkError::Codec)?;
        Ok(Self::new_encoded(id, method, msg_type, bytes))
    }

    /// JSON で payload をデコード
    pub fn payload_as_value(&self) -> Result<serde_json::Value, NetworkError> {
        Ok(<serde_json::Value as Decodable<JsonCodec>>::decode(
            &self.payload,
        )?)
    }

    /// 任意の Codec + 型で payload をデコード
    pub fn decode_payload<T, C: crate::codec::Codec>(&self) -> Result<T, NetworkError>
    where
        T: Decodable<C>,
    {
        Ok(T::decode(&self.payload)?)
    }
}

/// メッセージ種別
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Archive,
    RkyvSerialize,
    RkyvDeserialize,
)]
#[archive(check_bytes)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Request,
    Response,
    /// 一方向プッシュ（応答不要）
    Event,
    Error,
}

/// プロトコルエラー
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: i32,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `NetworkError::is_normal_close()` が `UnisonChannel::recv()` /
    /// `recv_raw()` / `request()` で生成される 3 種類の正常終端を判定し、
    /// それ以外の Protocol error / 他 variant は real error 扱いするか確認。
    #[test]
    fn is_normal_close_recognizes_channel_eof() {
        // recv() の end-of-stream
        assert!(NetworkError::Protocol("Channel closed".to_string()).is_normal_close());
        // recv_raw() の end-of-stream
        assert!(NetworkError::Protocol("Raw channel closed".to_string()).is_normal_close());
        // request() 中の oneshot sender drop
        assert!(
            NetworkError::Protocol("Request cancelled: channel closed".to_string())
                .is_normal_close()
        );
    }

    #[test]
    fn is_normal_close_rejects_other_errors() {
        // 他 Protocol error は real failure
        assert!(
            !NetworkError::Protocol("Failed to send channel open: io".to_string())
                .is_normal_close()
        );
        assert!(
            !NetworkError::Protocol("Failed to parse identity: bad json".to_string())
                .is_normal_close()
        );
        // 別 variant
        assert!(!NetworkError::Connection("conn refused".to_string()).is_normal_close());
        assert!(!NetworkError::Quic("transport".to_string()).is_normal_close());
        assert!(!NetworkError::Timeout.is_normal_close());
        assert!(!NetworkError::NotConnected.is_normal_close());
        assert!(
            !NetworkError::HandlerNotFound {
                method: "x".to_string()
            }
            .is_normal_close()
        );
    }
}
