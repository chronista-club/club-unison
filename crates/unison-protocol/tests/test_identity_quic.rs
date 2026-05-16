use unison::network::identity::*;

#[tokio::test]
async fn test_identity_channel_flow() {
    // サーバーがIdentityを構築
    let identity = ServerIdentity::new("test-server", "1.0.0", "test.ns");
    assert_eq!(identity.name, "test-server");
    assert_eq!(identity.version, "1.0.0");
    assert_eq!(identity.namespace, "test.ns");
    assert!(identity.channels.is_empty());

    // チャネルを追加
    let mut identity = identity;
    identity.add_channel(ChannelInfo {
        name: "events".to_string(),
        direction: ChannelDirection::ServerToClient,
        lifetime: "persistent".to_string(),
        status: ChannelStatus::Available,
    });
    assert_eq!(identity.channels.len(), 1);
    assert_eq!(identity.channels[0].name, "events");

    // ProtocolMessageに変換
    let msg = identity.to_protocol_message();
    assert_eq!(msg.method, "__identity");

    // ProtocolMessageから復元
    let restored = ServerIdentity::from_protocol_message(&msg).unwrap();
    assert_eq!(restored.name, "test-server");
    assert_eq!(restored.version, "1.0.0");
    assert_eq!(restored.channels.len(), 1);
    assert_eq!(restored.channels[0].name, "events");
}

#[test]
fn test_channel_update_serialization() {
    let update = ChannelUpdate::Added(ChannelInfo {
        name: "new-channel".to_string(),
        direction: ChannelDirection::Bidirectional,
        lifetime: "persistent".to_string(),
        status: ChannelStatus::Available,
    });

    let json = serde_json::to_string(&update).unwrap();
    let restored: ChannelUpdate = serde_json::from_str(&json).unwrap();

    match restored {
        ChannelUpdate::Added(info) => {
            assert_eq!(info.name, "new-channel");
            assert_eq!(info.direction, ChannelDirection::Bidirectional);
        }
        _ => panic!("Expected ChannelUpdate::Added"),
    }
}
