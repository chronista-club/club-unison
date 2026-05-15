mod common;

use club_unison::network::MessageType;
use club_unison::network::identity::*;

#[test]
fn test_integ_identity_to_protocol_message_round_trip() {
    let identity = common::make_identity("test-server", &["events", "query"]);
    let msg = identity.to_protocol_message();

    assert_eq!(msg.method, "__identity");
    assert_eq!(msg.msg_type, MessageType::Event);

    let restored = ServerIdentity::from_protocol_message(&msg).unwrap();
    assert_eq!(restored.name, "test-server");
    assert_eq!(restored.channels.len(), 2);
}

#[tokio::test]
async fn test_integ_identity_build_and_frame() {
    use club_unison::network::ProtocolServer;

    let server = ProtocolServer::with_identity("test-srv", "1.0.0", "ns");
    server
        .register_channel("ch1", |_ctx, _stream| async { Ok(()) })
        .await;

    // build_identity → to_protocol_message → into_frame の変換チェーン
    let identity = server.build_identity().await;
    assert_eq!(identity.name, "test-srv");
    assert_eq!(identity.channels.len(), 1);

    let msg = identity.to_protocol_message();
    assert_eq!(msg.method, "__identity");
    assert_eq!(msg.msg_type, MessageType::Event);

    // ProtocolMessage の payload JSON から復元できること
    let restored = ServerIdentity::from_protocol_message(&msg).unwrap();
    assert_eq!(restored.name, "test-srv");
    assert_eq!(restored.version, "1.0.0");
    assert_eq!(restored.namespace, "ns");
    assert_eq!(restored.channels.len(), 1);
    assert_eq!(restored.channels[0].name, "ch1");

    // フレーム化してバイト列が生成できること
    // v0.9.0 buffa pivot 後は header が variable-size なので、 fixed-size 比較ではなく
    // 「先頭 4 byte (= header_len prefix) + header body + payload」 が乗っていることを確認
    let frame = msg.into_frame().unwrap();
    let bytes = frame.to_bytes();
    assert!(bytes.len() > 4, "packet should at least contain header_len prefix");
    let header = frame.header().unwrap();
    assert!(header.payload_length > 0, "identity payload should be non-empty");
}

#[test]
fn test_integ_channel_update_variants_json() {
    let added = ChannelUpdate::Added(ChannelInfo {
        name: "new-ch".to_string(),
        direction: ChannelDirection::Bidirectional,
        lifetime: "persistent".to_string(),
        status: ChannelStatus::Available,
    });
    let json = serde_json::to_string(&added).unwrap();
    let restored: ChannelUpdate = serde_json::from_str(&json).unwrap();
    match restored {
        ChannelUpdate::Added(ch) => assert_eq!(ch.name, "new-ch"),
        _ => panic!("Expected Added variant"),
    }

    let removed = ChannelUpdate::Removed("old-ch".to_string());
    let json = serde_json::to_string(&removed).unwrap();
    let restored: ChannelUpdate = serde_json::from_str(&json).unwrap();
    match restored {
        ChannelUpdate::Removed(name) => assert_eq!(name, "old-ch"),
        _ => panic!("Expected Removed variant"),
    }

    let status_changed = ChannelUpdate::StatusChanged {
        name: "busy-ch".to_string(),
        status: ChannelStatus::Busy,
    };
    let json = serde_json::to_string(&status_changed).unwrap();
    let restored: ChannelUpdate = serde_json::from_str(&json).unwrap();
    match restored {
        ChannelUpdate::StatusChanged { name, status } => {
            assert_eq!(name, "busy-ch");
            assert_eq!(status, ChannelStatus::Busy);
        }
        _ => panic!("Expected StatusChanged variant"),
    }
}

#[tokio::test]
async fn test_integ_connection_context_identity_flow() {
    use club_unison::network::context::ConnectionContext;

    let ctx = ConnectionContext::new();
    assert!(ctx.identity().await.is_none());

    let identity = common::make_identity("ctx-server", &["data"]);
    ctx.set_identity(identity).await;

    let retrieved = ctx.identity().await.unwrap();
    assert_eq!(retrieved.name, "ctx-server");
    assert_eq!(retrieved.channels.len(), 1);
}
