//! Codec: アプリケーションメッセージのシリアライゼーション抽象化
//!
//! ## 概要
//!
//! `Codec` マーカートレイトと `Encodable<C>` / `Decodable<C>` トレイトペアで、
//! JSON / protobuf (buffa) 等のフォーマットを型安全に差し替え可能にする。
//!
//! ## 使い方
//!
//! ```rust,ignore
//! // JSON (デフォルト) — serde::Serialize/Deserialize な型はすべて使える
//! let channel: UnisonChannel<JsonCodec> = ...;
//! let resp: MyResponse = channel.request("method", &my_request).await?;
//!
//! // Protobuf — buffa::Message な型 (= buffa-build で生成した型) はすべて使える
//! let channel: UnisonChannel<ProtoCodec> = ...;
//! let resp: MyAck = channel.request("subscribe", &MySubscribe { ... }).await?;
//! ```

use thiserror::Error;

/// Codec エラー型
#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Encode error: {0}")]
    Encode(String),
    #[error("Decode error: {0}")]
    Decode(String),
}

/// Codec マーカートレイト
///
/// `UnisonChannel<C: Codec>` の型パラメータとして使用。
/// 実際の encode/decode は `Encodable<C>` / `Decodable<C>` が担う。
pub trait Codec: Send + Sync + 'static {}

/// `C: Codec` に対して、自身をバイト列にエンコードできることを表す
pub trait Encodable<C: ?Sized> {
    fn encode(&self) -> Result<Vec<u8>, CodecError>;
}

/// `C: Codec` に対して、バイト列から自身をデコードできることを表す
pub trait Decodable<C: ?Sized>: Sized {
    fn decode(bytes: &[u8]) -> Result<Self, CodecError>;
}

// ============================================================
// JsonCodec
// ============================================================

/// JSON Codec — serde ベースのシリアライゼーション
pub struct JsonCodec;

impl Codec for JsonCodec {}

impl<T: serde::Serialize> Encodable<JsonCodec> for T {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        serde_json::to_vec(self).map_err(|e| CodecError::Encode(e.to_string()))
    }
}

impl<T: serde::de::DeserializeOwned> Decodable<JsonCodec> for T {
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        serde_json::from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}

// ============================================================
// ProtoCodec
// ============================================================

/// Protobuf Codec — buffa ベースのシリアライゼーション
pub struct ProtoCodec;

impl Codec for ProtoCodec {}

impl<T: buffa::Message> Encodable<ProtoCodec> for T {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        Ok(self.encode_to_vec())
    }
}

impl<T: buffa::Message + Default> Decodable<ProtoCodec> for T {
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        T::decode_from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // ProtoCodec のテスト題材は core wire の buffa 生成型 (= 本 crate 唯一の .proto)
    use crate::proto::{MessageType, PacketHeader, ProtocolMessage};

    fn sample_message() -> ProtocolMessage {
        ProtocolMessage {
            id: 42,
            method: "subscribe".into(),
            msg_type: ::buffa::EnumValue::Known(MessageType::REQUEST),
            payload: b"a,b".to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn test_json_codec_value_roundtrip() {
        let value = serde_json::json!({
            "name": "test",
            "count": 42,
            "nested": { "items": [1, 2, 3] }
        });

        let encoded = Encodable::<JsonCodec>::encode(&value).unwrap();
        let decoded: serde_json::Value = Decodable::<JsonCodec>::decode(&encoded).unwrap();
        assert_eq!(value, decoded);
    }

    #[test]
    fn test_json_codec_typed_roundtrip() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct Ping {
            message: String,
        }

        let ping = Ping {
            message: "hello".into(),
        };

        let encoded = Encodable::<JsonCodec>::encode(&ping).unwrap();
        let decoded: Ping = Decodable::<JsonCodec>::decode(&encoded).unwrap();
        assert_eq!(ping, decoded);
    }

    #[test]
    fn test_json_codec_decode_error() {
        let result = <serde_json::Value as Decodable<JsonCodec>>::decode(b"not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_json_codec_empty_bytes_decode_error() {
        let result = <serde_json::Value as Decodable<JsonCodec>>::decode(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_codec_null_roundtrip() {
        let value = serde_json::Value::Null;
        let encoded = Encodable::<JsonCodec>::encode(&value).unwrap();
        let decoded: serde_json::Value = Decodable::<JsonCodec>::decode(&encoded).unwrap();
        assert_eq!(decoded, serde_json::Value::Null);
    }

    #[test]
    fn test_proto_codec_roundtrip() {
        let msg = sample_message();
        let encoded = Encodable::<ProtoCodec>::encode(&msg).unwrap();
        let decoded: ProtocolMessage = Decodable::<ProtoCodec>::decode(&encoded).unwrap();
        assert_eq!(decoded.id, 42);
        assert_eq!(decoded.method, "subscribe");
        assert_eq!(decoded.payload, b"a,b");
    }

    #[test]
    fn test_proto_codec_decode_error() {
        let result = <PacketHeader as Decodable<ProtoCodec>>::decode(&[0xFF, 0xFF, 0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn test_proto_codec_large_message() {
        let msg = ProtocolMessage {
            payload: vec![b'x'; 10_000],
            ..sample_message()
        };
        let encoded = Encodable::<ProtoCodec>::encode(&msg).unwrap();
        let decoded: ProtocolMessage = Decodable::<ProtoCodec>::decode(&encoded).unwrap();
        assert_eq!(decoded.payload.len(), 10_000);
        assert_eq!(decoded.method, "subscribe");
    }

    #[test]
    fn test_proto_codec_empty_message() {
        // proto3 のデフォルト値はゼロバイトにエンコードされる
        let msg = PacketHeader::default();
        let encoded = Encodable::<ProtoCodec>::encode(&msg).unwrap();
        assert!(encoded.is_empty());
        let decoded: PacketHeader = Decodable::<ProtoCodec>::decode(&encoded).unwrap();
        assert_eq!(decoded.version, 0);
    }

    #[test]
    fn test_json_and_proto_encode_different_bytes() {
        // 同じ論理データでも JsonCodec と ProtoCodec では異なるバイト列になる
        let msg = sample_message();
        let proto_bytes = Encodable::<ProtoCodec>::encode(&msg).unwrap();

        let json_value = serde_json::json!({"id": 42, "method": "subscribe", "payload": "a,b"});
        let json_bytes = Encodable::<JsonCodec>::encode(&json_value).unwrap();

        assert_ne!(proto_bytes, json_bytes);
        // protobuf のほうがコンパクト
        assert!(proto_bytes.len() < json_bytes.len());
    }

    #[test]
    fn test_proto_codec_truncated_bytes_decode_error() {
        let encoded = Encodable::<ProtoCodec>::encode(&sample_message()).unwrap();
        // 末尾を切り落とした truncated バイト列
        let truncated = &encoded[..encoded.len() / 2];
        let result = <ProtocolMessage as Decodable<ProtoCodec>>::decode(truncated);
        assert!(result.is_err());
    }

    #[test]
    fn test_codec_error_display() {
        let enc_err = CodecError::Encode("test encode error".to_string());
        assert!(enc_err.to_string().contains("test encode error"));

        let dec_err = CodecError::Decode("test decode error".to_string());
        assert!(dec_err.to_string().contains("test decode error"));
    }
}
