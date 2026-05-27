use ::buffa::Message;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::codec::{CodecError, Decodable, Encodable, JsonCodec};
use crate::packet::{SerializationError, UnisonPacket};
use crate::proto;

pub mod cert;
pub mod channel;
pub mod client;
pub mod conn;
pub mod conn_quinn;
pub mod context;
pub mod datagram_channel;
pub mod datagram_dispatcher;
pub mod discovery;
pub mod dispatch;
pub mod frame;
pub mod identity;
pub mod mesh;
pub mod protocol_cache;
pub mod quic;
pub mod server;
pub mod stream;
pub mod trust;
pub mod webtransport;

pub use cert::CertSource;
pub use channel::UnisonChannel;
pub use client::{ClientConnectionEvent, ClientConnectionEventReceiver, ProtocolClient};
pub use conn::UnisonConn;
pub use datagram_channel::DatagramChannel;
pub use discovery::{
    DISCOVERY_CHANNEL_NAME, GET_PROTOCOL_METHOD, GetProtocolRequest, ProtocolDocument,
    SCHEMA_UPDATED_EVENT, SchemaUpdatedEvent,
};
pub use mesh::{InternalMeshKeypair, MeshCa};
pub use protocol_cache::ProtocolCache;
pub use quic::{QuicClient, QuicServer, TypedFrame, UnisonStream};
pub use server::{ConnectionEvent, ConnectionEventReceiver, ProtocolServer, ServerHandle};
pub use trust::TrustAnchors;
pub use webtransport::WebTransportServer;

/// グローバルなリクエストID生成（モジュール間で一意）
pub(crate) fn generate_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// エラーの分類
///
/// boundary error を programmatic に判定可能にする (Phase 5 / UNS-15)。
/// caller は category で分岐し、retry 可否やログレベルを決められる。
/// TS SDK 側の `ErrorCategory` と value (snake_case 文字列) を一致させること。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// トランスポート層 (QUIC / TLS / DNS)
    Transport,
    /// プロトコル層 (不正パケット / スキーマ不整合 / チャネル状態)
    Protocol,
    /// アプリケーション層 (caller / handler が返したエラー)
    Application,
    /// リソース層 (quota / rate-limit / timeout)
    Resource,
}

impl ErrorCategory {
    /// snake_case の文字列表現 (TS SDK の値と一致)
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCategory::Transport => "transport",
            ErrorCategory::Protocol => "protocol",
            ErrorCategory::Application => "application",
            ErrorCategory::Resource => "resource",
        }
    }
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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

    /// この error の分類を返す (Phase 5 / UNS-15)。
    ///
    /// 各 variant を `ErrorCategory` の 4 区分に写像する。caller は category で
    /// retry 可否やログレベルを決められる。
    pub fn category(&self) -> ErrorCategory {
        match self {
            // トランスポート層: QUIC / 接続 / トランスポート種別
            NetworkError::Connection(_)
            | NetworkError::Quic(_)
            | NetworkError::NotConnected
            | NetworkError::UnsupportedTransport(_) => ErrorCategory::Transport,
            // プロトコル層: 不正パケット / スキーマ不整合 / チャネル状態 / シリアライズ
            NetworkError::Protocol(_)
            | NetworkError::Serialization(_)
            | NetworkError::Codec(_)
            | NetworkError::FrameSerialization(_) => ErrorCategory::Protocol,
            // アプリケーション層: handler が見つからない (caller 指定ミス)
            NetworkError::HandlerNotFound { .. } => ErrorCategory::Application,
            // リソース層: timeout (quota / rate-limit もここに将来追加)
            NetworkError::Timeout => ErrorCategory::Resource,
        }
    }
}

/// プロトコルメッセージラッパー
///
/// wire 上は buffa-encoded `proto::ProtocolMessage` として運ばれる。
/// 本 struct はその等価表現 (= PascalCase enum / 直 field access) を提供。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessage {
    pub id: u64,
    pub method: String,
    #[serde(rename = "type")]
    pub msg_type: MessageType,
    pub payload: Vec<u8>, // Codec がエンコードしたバイト列
}

/// フレームでラップされたプロトコルメッセージの型エイリアス
///
/// v0.9.0 buffa pivot 後は `UnisonPacket` 自体が非ジェネリック (= 生バイト保持)
/// になったため、 ProtocolFrame は単なるエイリアス。
pub type ProtocolFrame = UnisonPacket;

impl ProtocolMessage {
    /// ProtocolMessage をフレームに変換
    ///
    /// 内部で buffa の `proto::ProtocolMessage` にエンコードしたのち
    /// `UnisonPacket` (= packet header + payload bytes) で包む。
    pub fn into_frame(self) -> Result<ProtocolFrame, SerializationError> {
        let proto_msg = self.into_proto();
        let payload_bytes = proto_msg.encode_to_vec();
        UnisonPacket::new(payload_bytes)
    }

    /// フレームから ProtocolMessage を復元
    pub fn from_frame(frame: &ProtocolFrame) -> Result<Self, SerializationError> {
        let payload_bytes = frame.payload()?;
        let proto_msg = proto::ProtocolMessage::decode_from_slice(&payload_bytes)
            .map_err(|e| SerializationError::DeserializationFailed(e.to_string()))?;
        Ok(Self::from_proto(proto_msg))
    }

    /// エンコード済みバイト列から ProtocolMessage を直接作成
    pub fn new_encoded(id: u64, method: String, msg_type: MessageType, payload: Vec<u8>) -> Self {
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
        let bytes = Encodable::<JsonCodec>::encode(&payload).map_err(NetworkError::Codec)?;
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

    /// 内部: buffa `proto::ProtocolMessage` への変換 (encoding 用)
    fn into_proto(self) -> proto::ProtocolMessage {
        proto::ProtocolMessage {
            id: self.id,
            method: self.method,
            msg_type: ::buffa::EnumValue::Known(self.msg_type.to_proto()),
            payload: self.payload,
            __buffa_unknown_fields: Default::default(),
        }
    }

    /// 内部: buffa `proto::ProtocolMessage` からの復元 (decoding 用)
    ///
    /// 未知の MessageType 値が wire 上に乗っていた場合は `MessageType::Error`
    /// として扱う (= caller は msg_type で分岐できる、 wire 互換性は維持)。
    fn from_proto(p: proto::ProtocolMessage) -> Self {
        let msg_type = p
            .msg_type
            .as_known()
            .map(MessageType::from_proto)
            .unwrap_or(MessageType::Error);
        Self {
            id: p.id,
            method: p.method,
            msg_type,
            payload: p.payload,
        }
    }
}

/// メッセージ種別
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Request,
    Response,
    /// 一方向プッシュ（応答不要）
    Event,
    Error,
}

impl MessageType {
    fn to_proto(self) -> proto::MessageType {
        match self {
            MessageType::Request => proto::MessageType::REQUEST,
            MessageType::Response => proto::MessageType::RESPONSE,
            MessageType::Event => proto::MessageType::EVENT,
            MessageType::Error => proto::MessageType::ERROR,
        }
    }

    fn from_proto(p: proto::MessageType) -> Self {
        match p {
            proto::MessageType::REQUEST => MessageType::Request,
            proto::MessageType::RESPONSE => MessageType::Response,
            proto::MessageType::EVENT => MessageType::Event,
            proto::MessageType::ERROR => MessageType::Error,
        }
    }
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

    /// ProtocolMessage の buffa wire round-trip を確認
    #[test]
    fn protocol_message_proto_round_trip() {
        let original = ProtocolMessage::new_encoded(
            42,
            "test.method".to_string(),
            MessageType::Request,
            b"payload".to_vec(),
        );

        let frame = original.clone().into_frame().unwrap();
        let bytes = frame.to_bytes();
        let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
        let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

        assert_eq!(restored.id, original.id);
        assert_eq!(restored.method, original.method);
        assert_eq!(restored.msg_type, original.msg_type);
        assert_eq!(restored.payload, original.payload);
    }

    /// 全 NetworkError variant が想定どおりの ErrorCategory に写像されること
    #[test]
    fn network_error_category_mapping() {
        use ErrorCategory::*;
        let cases: &[(NetworkError, ErrorCategory)] = &[
            (NetworkError::Connection("x".into()), Transport),
            (NetworkError::Quic("x".into()), Transport),
            (NetworkError::NotConnected, Transport),
            (NetworkError::UnsupportedTransport("x".into()), Transport),
            (NetworkError::Protocol("x".into()), Protocol),
            (
                NetworkError::FrameSerialization(SerializationError::InvalidHeader),
                Protocol,
            ),
            (
                NetworkError::HandlerNotFound { method: "x".into() },
                Application,
            ),
            (NetworkError::Timeout, Resource),
        ];
        for (err, expected) in cases {
            assert_eq!(err.category(), *expected, "{err:?}");
        }
    }

    /// ErrorCategory の文字列表現が TS SDK の値と一致すること
    #[test]
    fn error_category_str_values() {
        assert_eq!(ErrorCategory::Transport.as_str(), "transport");
        assert_eq!(ErrorCategory::Protocol.as_str(), "protocol");
        assert_eq!(ErrorCategory::Application.as_str(), "application");
        assert_eq!(ErrorCategory::Resource.as_str(), "resource");
    }

    /// 各 MessageType variant が wire を通って同じ variant で戻ること
    #[test]
    fn message_type_proto_round_trip_all_variants() {
        for variant in [
            MessageType::Request,
            MessageType::Response,
            MessageType::Event,
            MessageType::Error,
        ] {
            let p = variant.to_proto();
            assert_eq!(MessageType::from_proto(p), variant);
        }
    }
}
