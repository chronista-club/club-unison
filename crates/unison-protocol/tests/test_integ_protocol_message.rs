mod common;

use club_unison::network::{MessageType, ProtocolMessage};
use club_unison::packet::UnisonPacket;

/// Request型 ProtocolMessage の into_frame → to_bytes → from_bytes → from_frame 往復
#[test]
fn test_integ_request_round_trip() {
    let msg = ProtocolMessage::new_with_json(
        42,
        "test.method".to_string(),
        MessageType::Request,
        serde_json::json!({"key": "value"}),
    )
    .unwrap();

    let frame = msg.clone().into_frame().unwrap();
    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);
    assert_eq!(restored.payload, msg.payload);
}

/// Response型の往復
#[test]
fn test_integ_response_round_trip() {
    let msg = ProtocolMessage::new_with_json(
        99,
        "test.response".to_string(),
        MessageType::Response,
        serde_json::json!({"status": "ok", "data": [1, 2, 3]}),
    )
    .unwrap();

    let frame = msg.clone().into_frame().unwrap();
    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);
    assert_eq!(restored.payload, msg.payload);
}

/// Event型の往復
#[test]
fn test_integ_event_round_trip() {
    let msg = common::make_event(
        "system.notify",
        serde_json::json!({"level": "info", "msg": "hello"}),
    );

    let frame = msg.clone().into_frame().unwrap();
    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);
    assert_eq!(restored.payload, msg.payload);
}

/// Error型の往復
#[test]
fn test_integ_error_round_trip() {
    let msg = ProtocolMessage::new_with_json(
        7,
        "test.error".to_string(),
        MessageType::Error,
        serde_json::json!({"code": 404, "message": "not found"}),
    )
    .unwrap();

    let frame = msg.clone().into_frame().unwrap();
    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);
    assert_eq!(restored.payload, msg.payload);
}

/// ネストしたJSON、配列、Unicode文字列での往復
#[test]
fn test_integ_complex_json_round_trip() {
    let complex_payload = serde_json::json!({
        "user": {
            "name": "田中太郎",
            "tags": ["admin", "developer"],
            "profile": {
                "bio": "こんにちは世界 🌍",
                "scores": [100, 200, 300]
            }
        },
        "metadata": {
            "nested": {
                "deep": {
                    "value": true
                }
            }
        },
        "emoji": "🎉🚀✨",
        "empty_array": [],
        "null_field": null
    });

    let msg = common::make_request("complex.method", complex_payload);

    let frame = msg.clone().into_frame().unwrap();
    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);

    // JSON値として比較（キー順序に依存しない）
    let original_value: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let restored_value: serde_json::Value = serde_json::from_slice(&restored.payload).unwrap();
    assert_eq!(original_value, restored_value);
}

/// 2KB超のペイロードが自動圧縮されてから往復
#[test]
fn test_integ_large_payload_compression_round_trip() {
    // 圧縮閾値（2048バイト）を超えるペイロードを生成
    let large_data: Vec<serde_json::Value> = (0..200)
        .map(|i| {
            serde_json::json!({
                "index": i,
                "value": format!("item_{:04}", i),
                "description": "This is a test entry for compression verification"
            })
        })
        .collect();

    let msg = common::make_request("bulk.data", serde_json::json!({"items": large_data}));

    // ペイロードが閾値を超えていることを確認
    assert!(
        msg.payload.len() > 2048,
        "payload should exceed compression threshold"
    );

    let frame = msg.clone().into_frame().unwrap();

    // 圧縮が適用されていることを確認
    let header = frame.header().unwrap();
    assert!(header.is_compressed(), "large payload should be compressed");

    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);

    let original_value: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    let restored_value: serde_json::Value = serde_json::from_slice(&restored.payload).unwrap();
    assert_eq!(original_value, restored_value);
}

/// 空ペイロード（`{}`）での往復
#[test]
fn test_integ_empty_payload_round_trip() {
    let msg = common::make_message("empty.method", MessageType::Request, serde_json::json!({}));

    let frame = msg.clone().into_frame().unwrap();
    let bytes = frame.to_bytes();

    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, msg.id);
    assert_eq!(restored.method, msg.method);
    assert_eq!(restored.msg_type, msg.msg_type);
    assert_eq!(restored.payload, msg.payload);
}

/// new_encoded で直接バイト列から ProtocolMessage を作成
#[test]
fn test_integ_new_encoded() {
    let payload_bytes = serde_json::to_vec(&serde_json::json!({"key": "value"})).unwrap();

    let msg = ProtocolMessage::new_encoded(
        1,
        "test.method".to_string(),
        MessageType::Request,
        payload_bytes.clone(),
    );

    assert_eq!(msg.id, 1);
    assert_eq!(msg.method, "test.method");
    assert_eq!(msg.payload, payload_bytes);
}

/// decode_payload で JsonCodec 経由のデコード
#[test]
fn test_integ_decode_payload_json() {
    use club_unison::codec::JsonCodec;

    let original = serde_json::json!({"name": "test", "count": 42});
    let msg = ProtocolMessage::new_with_json(
        1,
        "test".to_string(),
        MessageType::Request,
        original.clone(),
    )
    .unwrap();

    let decoded: serde_json::Value = msg.decode_payload::<_, JsonCodec>().unwrap();
    assert_eq!(decoded, original);
}

/// decode_payload で ProtoCodec 経由のデコード
#[test]
fn test_integ_decode_payload_proto() {
    use buffa::Message;
    use club_unison::codec::ProtoCodec;
    use club_unison::codec::proto::creo_sync::Subscribe;

    let subscribe = Subscribe {
        category: "design".into(),
        tags: "arch".into(),
        ..Default::default()
    };

    let msg = ProtocolMessage::new_encoded(
        1,
        "subscribe".to_string(),
        MessageType::Request,
        subscribe.encode_to_vec(),
    );

    let decoded: Subscribe = msg.decode_payload::<_, ProtoCodec>().unwrap();
    assert_eq!(decoded.category, "design");
    assert_eq!(decoded.tags, "arch");
}

/// new_encoded → into_frame → from_frame → decode_payload の一気通貫
#[test]
fn test_integ_proto_frame_roundtrip() {
    use buffa::Message;
    use club_unison::codec::ProtoCodec;
    use club_unison::codec::proto::creo_sync::Ack;

    let ack = Ack {
        status: "ok".into(),
        channel_ref: "ctrl-1".into(),
        ..Default::default()
    };

    let msg = ProtocolMessage::new_encoded(
        42,
        "ack".to_string(),
        MessageType::Response,
        ack.encode_to_vec(),
    );

    // フレーム化 → バイト列 → 復元
    let frame = msg.into_frame().unwrap();
    let bytes = frame.to_bytes();
    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    // ProtoCodec でデコード
    let decoded: Ack = restored.decode_payload::<_, ProtoCodec>().unwrap();
    assert_eq!(decoded.status, "ok");
    assert_eq!(decoded.channel_ref, "ctrl-1");
}

/// payload_as_value は proto バイト列に対してエラーを返すこと
#[test]
fn test_integ_payload_as_value_with_proto_bytes_fails() {
    use buffa::Message;
    use club_unison::codec::proto::creo_sync::Subscribe;

    let subscribe = Subscribe {
        category: "test".into(),
        tags: "a,b".into(),
        ..Default::default()
    };

    let msg = ProtocolMessage::new_encoded(
        1,
        "subscribe".to_string(),
        MessageType::Request,
        subscribe.encode_to_vec(),
    );

    // payload_as_value() は JSON を仮定するので proto bytes ではエラー
    let result = msg.payload_as_value();
    assert!(result.is_err());
}

/// JSON でエンコードした payload を ProtoCodec でデコードするとエラー（Codec 混用検出）
#[test]
fn test_integ_decode_payload_wrong_codec_json_to_proto() {
    use club_unison::codec::ProtoCodec;
    use club_unison::codec::proto::creo_sync::Subscribe;

    let msg = ProtocolMessage::new_with_json(
        1,
        "test".to_string(),
        MessageType::Request,
        serde_json::json!({"category": "test", "tags": "a"}),
    )
    .unwrap();

    // JSON bytes を ProtoCodec (buffa) でデコード → エラー
    let result = msg.decode_payload::<Subscribe, ProtoCodec>();
    assert!(result.is_err());
}

/// Proto でエンコードした payload を JsonCodec でデコードするとエラー（逆方向の混用検出）
#[test]
fn test_integ_decode_payload_wrong_codec_proto_to_json() {
    use buffa::Message;
    use club_unison::codec::JsonCodec;
    use club_unison::codec::proto::creo_sync::Subscribe;

    let subscribe = Subscribe {
        category: "test".into(),
        tags: "a,b".into(),
        ..Default::default()
    };

    let msg = ProtocolMessage::new_encoded(
        1,
        "test".to_string(),
        MessageType::Request,
        subscribe.encode_to_vec(),
    );

    // Proto bytes を JsonCodec でデコード → エラー
    let result = msg.decode_payload::<serde_json::Value, JsonCodec>();
    assert!(result.is_err());
}

/// new_encoded は任意のバイナリをそのまま保存すること
#[test]
fn test_integ_new_encoded_preserves_arbitrary_bytes() {
    let arbitrary = vec![0x00, 0x01, 0xFE, 0xFF, 0x80, 0x7F];

    let msg = ProtocolMessage::new_encoded(
        1,
        "raw".to_string(),
        MessageType::Request,
        arbitrary.clone(),
    );

    assert_eq!(msg.payload, arbitrary);
}

/// 全 MessageType バリアントで new_encoded が動作すること
#[test]
fn test_integ_new_encoded_all_message_types() {
    let payload = b"test".to_vec();

    for msg_type in [
        MessageType::Request,
        MessageType::Response,
        MessageType::Event,
        MessageType::Error,
    ] {
        let msg = ProtocolMessage::new_encoded(1, "test".to_string(), msg_type, payload.clone());
        assert_eq!(msg.msg_type, msg_type);
        assert_eq!(msg.payload, payload);
    }
}

/// new_with_json → into_frame → from_frame 後も payload_as_value が正しく復元できること
#[test]
fn test_integ_new_with_json_encodes_valid_json() {
    let original = serde_json::json!({"key": "value", "num": 123});

    let msg = ProtocolMessage::new_with_json(
        1,
        "test".to_string(),
        MessageType::Request,
        original.clone(),
    )
    .unwrap();

    // payload バイト列が有効な JSON であること
    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(parsed, original);
}

/// Proto 大ペイロードの圧縮 roundtrip
#[test]
fn test_integ_proto_large_payload_compression_roundtrip() {
    use buffa::Message;
    use club_unison::codec::ProtoCodec;
    use club_unison::codec::proto::creo_sync::MemoryEvent;

    // 圧縮閾値 (2048B) を超える proto ペイロードを生成
    let msg = MemoryEvent {
        event_type: "bulk_import".into(),
        memory_id: "mem_large".into(),
        category: "x".repeat(5000),
        from: "importer".into(),
        timestamp: "2026-03-28T00:00:00Z".into(),
        ..Default::default()
    };

    let proto_bytes = msg.encode_to_vec();
    assert!(
        proto_bytes.len() > 2048,
        "payload should exceed compression threshold"
    );

    let protocol_msg =
        ProtocolMessage::new_encoded(1, "import".to_string(), MessageType::Event, proto_bytes);

    // フレーム化（圧縮される）→ バイト列 → 復元
    let frame = protocol_msg.into_frame().unwrap();
    let header = frame.header().unwrap();
    assert!(
        header.is_compressed(),
        "large proto payload should be compressed"
    );

    let bytes = frame.to_bytes();
    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    // ProtoCodec でデコードして検証
    let decoded: MemoryEvent = restored.decode_payload::<_, ProtoCodec>().unwrap();
    assert_eq!(decoded.event_type, "bulk_import");
    assert_eq!(decoded.category.len(), 5000);
}

/// id=0 の ProtocolMessage がフレーム往復後も id=0 で戻ること
#[test]
fn test_integ_zero_id_roundtrip() {
    let msg =
        ProtocolMessage::new_encoded(0, "event".to_string(), MessageType::Event, b"{}".to_vec());

    let frame = msg.into_frame().unwrap();
    let bytes = frame.to_bytes();
    let restored_frame = UnisonPacket::from_bytes(&bytes).unwrap();
    let restored = ProtocolMessage::from_frame(&restored_frame).unwrap();

    assert_eq!(restored.id, 0);
}
