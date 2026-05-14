mod common;

use bytes::Bytes;
use club_unison::context::{HandlerRegistry, MessageDispatcher};
use club_unison::network::{MessageType, NetworkError, ProtocolMessage};
use club_unison::packet::config::{CompressionConfig, PacketConfig};
use club_unison::packet::header::{PacketType, UnisonPacketHeader};
use club_unison::packet::payload::RkyvPayload;
use club_unison::packet::serialization::PacketSerializer;
use club_unison::packet::{SerializationError, UnisonPacket};

/// 56バイト未満のバイト列 → from_bytes がエラー
#[test]
fn test_integ_frame_too_short() {
    let short_bytes = Bytes::from(vec![0u8; 10]);
    let result = UnisonPacket::<RkyvPayload<ProtocolMessage>>::from_bytes(&short_bytes);
    assert!(result.is_err());
}

/// ランダムな56+バイト → ヘッダーパースエラー
#[test]
fn test_integ_frame_invalid_header() {
    let random_bytes = Bytes::from(vec![0xFFu8; 100]);
    let result = UnisonPacket::<RkyvPayload<ProtocolMessage>>::from_bytes(&random_bytes);
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
    let payload = RkyvPayload::new(msg);
    let mut header = UnisonPacketHeader::new(PacketType::Data);
    header.version = 0xFF; // 不正バージョン

    let frame = UnisonPacket::with_header(header, payload).unwrap();
    let bytes = frame.to_bytes();
    let result = UnisonPacket::<RkyvPayload<ProtocolMessage>>::from_bytes(&bytes);
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

/// HandlerRegistry で未登録メソッドに dispatch → NetworkError::HandlerNotFound
#[tokio::test]
async fn test_integ_handler_not_found_error() {
    let registry = HandlerRegistry::new();

    let msg = common::make_request("unknown_method", serde_json::json!({"key": "value"}));

    let result = registry.dispatch(msg).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        NetworkError::HandlerNotFound { method } => {
            assert_eq!(method, "unknown_method");
        }
        e => panic!("Expected HandlerNotFound, got: {:?}", e),
    }
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

    let payload = RkyvPayload::new(msg);
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
