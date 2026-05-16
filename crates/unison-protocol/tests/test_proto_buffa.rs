//! buffa 生成型の encode/decode テスト
//!
//! creo_sync.proto から生成された Rust 型が
//! buffa::Message トレイト経由で正しく往復できることを検証する。

use buffa::Message;
use unison::codec::proto::creo_sync::*;

#[test]
fn test_subscribe_roundtrip() {
    let msg = Subscribe {
        category: "design-decision".into(),
        tags: "unison,buffa".into(),
        ..Default::default()
    };

    // encode
    let bytes = msg.encode_to_vec();
    assert!(!bytes.is_empty());

    // decode
    let decoded = Subscribe::decode_from_slice(&bytes).unwrap();
    assert_eq!(decoded.category, "design-decision");
    assert_eq!(decoded.tags, "unison,buffa");
}

#[test]
fn test_ack_roundtrip() {
    let msg = Ack {
        status: "ok".into(),
        channel_ref: "control-1".into(),
        ..Default::default()
    };

    let bytes = msg.encode_to_vec();
    let decoded = Ack::decode_from_slice(&bytes).unwrap();
    assert_eq!(decoded.status, "ok");
    assert_eq!(decoded.channel_ref, "control-1");
}

#[test]
fn test_memory_event_roundtrip() {
    let msg = MemoryEvent {
        event_type: "created".into(),
        memory_id: "mem_abc123".into(),
        category: "design-decision".into(),
        from: "creo-lead".into(),
        timestamp: "2026-03-28T12:00:00Z".into(),
        ..Default::default()
    };

    let bytes = msg.encode_to_vec();
    let decoded = MemoryEvent::decode_from_slice(&bytes).unwrap();
    assert_eq!(decoded.event_type, "created");
    assert_eq!(decoded.memory_id, "mem_abc123");
    assert_eq!(decoded.timestamp, "2026-03-28T12:00:00Z");
}

#[test]
fn test_query_with_bytes_field() {
    let params = serde_json::json!({"limit": 10, "offset": 0});
    let params_bytes = serde_json::to_vec(&params).unwrap();

    let msg = Query {
        method: "search".into(),
        params: params_bytes.clone(),
        ..Default::default()
    };

    let bytes = msg.encode_to_vec();
    let decoded = Query::decode_from_slice(&bytes).unwrap();
    assert_eq!(decoded.method, "search");

    // bytes フィールドから JSON を復元
    let restored: serde_json::Value = serde_json::from_slice(&decoded.params).unwrap();
    assert_eq!(restored, params);
}

#[test]
fn test_empty_message_roundtrip() {
    // デフォルト値のみのメッセージも正しく往復できること
    let msg = Subscribe::default();
    let bytes = msg.encode_to_vec();
    let decoded = Subscribe::decode_from_slice(&bytes).unwrap();
    assert_eq!(decoded.category, "");
    assert_eq!(decoded.tags, "");
}

#[test]
fn test_protobuf_is_compact() {
    // protobuf が JSON よりコンパクトであることを確認
    let msg = MemoryEvent {
        event_type: "created".into(),
        memory_id: "mem_abc123".into(),
        category: "design-decision".into(),
        from: "creo-lead".into(),
        timestamp: "2026-03-28T12:00:00Z".into(),
        ..Default::default()
    };

    let proto_bytes = msg.encode_to_vec();
    let json_bytes = serde_json::to_vec(&serde_json::json!({
        "event_type": "created",
        "memory_id": "mem_abc123",
        "category": "design-decision",
        "from": "creo-lead",
        "timestamp": "2026-03-28T12:00:00Z"
    }))
    .unwrap();

    assert!(
        proto_bytes.len() < json_bytes.len(),
        "protobuf ({} bytes) should be more compact than JSON ({} bytes)",
        proto_bytes.len(),
        json_bytes.len()
    );
}
