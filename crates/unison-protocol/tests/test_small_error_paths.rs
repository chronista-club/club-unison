mod common;

use bytes::Bytes;
use unison::network::{MessageType, ProtocolMessage};
use unison::packet::config::{CompressionConfig, PacketConfig};
use unison::packet::header::{PacketType, UnisonPacketHeader};
use unison::packet::serialization::PacketSerializer;
use unison::packet::{SerializationError, UnisonPacket};

/// 短すぎるバイト列 → from_bytes が InvalidHeader を返す
#[test]
fn test_integ_frame_too_short() {
    let short_bytes = Bytes::from(vec![0u8; 3]); // u32 prefix 未満
    let result = UnisonPacket::from_bytes(&short_bytes);
    assert!(result.is_err());
}

/// ランダムバイト列 → ヘッダーパースエラー
#[test]
fn test_integ_frame_invalid_header() {
    let random_bytes = Bytes::from(vec![0xFFu8; 100]);
    let result = UnisonPacket::from_bytes(&random_bytes);
    assert!(result.is_err());
}

/// 不正バージョンのヘッダーでフレームを構築 → from_bytes で拒否
#[test]
fn test_integ_frame_version_mismatch() {
    let msg = ProtocolMessage::new_with_json(
        1,
        "test".to_string(),
        MessageType::Request,
        serde_json::json!({}),
    )
    .unwrap();

    // 不正バージョンのヘッダーでフレームを手動構築
    use ::buffa::Message;
    let proto_msg = proto_message_from(msg);
    let payload_bytes = proto_msg.encode_to_vec();
    let mut header = UnisonPacketHeader::new(PacketType::Data);
    header.version = 0xFF; // 不正バージョン

    let frame = UnisonPacket::with_header(header, payload_bytes).unwrap();
    let bytes = frame.to_bytes();
    let result = UnisonPacket::from_bytes(&bytes);
    assert!(result.is_err());
}

/// 不正JSON文字列での payload_as_value() エラー
#[test]
fn test_integ_invalid_json_payload() {
    let msg = ProtocolMessage {
        id: 1,
        method: "test".to_string(),
        msg_type: MessageType::Request,
        payload: b"this is not json {{{".to_vec(),
    };

    let result = msg.payload_as_value();
    assert!(result.is_err());
}

/// PacketConfig の max_payload_size を小さく設定して超過テスト
#[test]
fn test_integ_max_payload_size_exceeded() {
    let config = PacketConfig::new()
        .with_compression(CompressionConfig::disabled())
        .with_max_payload_size(100); // 非常に小さい制限

    let msg = ProtocolMessage::new_with_json(
        1,
        "test".to_string(),
        MessageType::Request,
        serde_json::json!({"data": "x".repeat(200)}),
    )
    .unwrap();

    use ::buffa::Message;
    let payload = proto_message_from(msg).encode_to_vec();
    let mut header = UnisonPacketHeader::new(PacketType::Data);
    let result = PacketSerializer::serialize_with_config(&mut header, &payload, &config);
    assert!(result.is_err());
    match result.unwrap_err() {
        SerializationError::PacketTooLarge { size, max_size } => {
            assert_eq!(max_size, 100);
            assert!(size > 100);
        }
        e => panic!("Expected PacketTooLarge, got: {:?}", e),
    }
}

// helper: ProtocolMessage を buffa proto::ProtocolMessage に変換
//
// `network::ProtocolMessage` は private な `into_proto()` を持つが、 test crate からは
// 触れないので、 ここで等価な変換を再現する (= 同じ wire format で encode するため)。
fn proto_message_from(msg: ProtocolMessage) -> unison::proto::ProtocolMessage {
    use unison::proto;
    let msg_type = match msg.msg_type {
        MessageType::Request => proto::MessageType::REQUEST,
        MessageType::Response => proto::MessageType::RESPONSE,
        MessageType::Event => proto::MessageType::EVENT,
        MessageType::Error => proto::MessageType::ERROR,
    };
    proto::ProtocolMessage {
        id: msg.id,
        method: msg.method,
        msg_type: ::buffa::EnumValue::Known(msg_type),
        payload: msg.payload,
        __buffa_unknown_fields: Default::default(),
    }
}
