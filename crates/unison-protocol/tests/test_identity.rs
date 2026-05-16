use unison::network::identity::*;

#[test]
fn test_identity_serialization() {
    let identity = ServerIdentity {
        name: "creo-memories".to_string(),
        version: "1.0.0".to_string(),
        namespace: "club.chronista.sync".to_string(),
        channels: vec![
            ChannelInfo {
                name: "events".to_string(),
                direction: ChannelDirection::ServerToClient,
                lifetime: "persistent".to_string(),
                status: ChannelStatus::Available,
            },
            ChannelInfo {
                name: "query".to_string(),
                direction: ChannelDirection::Bidirectional,
                lifetime: "transient".to_string(),
                status: ChannelStatus::Available,
            },
        ],
        metadata: serde_json::json!({
            "project": "creo-memories",
            "role": "memory-store"
        }),
    };

    let json = serde_json::to_string(&identity).unwrap();
    let deserialized: ServerIdentity = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "creo-memories");
    assert_eq!(deserialized.channels.len(), 2);
    assert_eq!(deserialized.channels[0].status, ChannelStatus::Available);
}

#[test]
fn test_channel_update() {
    let update = ChannelUpdate::Added(ChannelInfo {
        name: "alerts".to_string(),
        direction: ChannelDirection::ServerToClient,
        lifetime: "transient".to_string(),
        status: ChannelStatus::Available,
    });

    let json = serde_json::to_string(&update).unwrap();
    let deserialized: ChannelUpdate = serde_json::from_str(&json).unwrap();
    match deserialized {
        ChannelUpdate::Added(info) => assert_eq!(info.name, "alerts"),
        _ => panic!("Expected Added variant"),
    }
}
