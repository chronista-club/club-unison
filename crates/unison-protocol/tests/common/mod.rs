use unison::network::{MessageType, ProtocolMessage};

/// テスト用の ProtocolMessage を生成
#[allow(dead_code)]
pub fn make_message(
    method: &str,
    msg_type: MessageType,
    payload: serde_json::Value,
) -> ProtocolMessage {
    ProtocolMessage::new_with_json(1, method.to_string(), msg_type, payload).unwrap()
}

/// テスト用のリクエストメッセージを生成
#[allow(dead_code)]
pub fn make_request(method: &str, payload: serde_json::Value) -> ProtocolMessage {
    ProtocolMessage::new_with_json(1, method.to_string(), MessageType::Request, payload).unwrap()
}

/// テスト用のイベントメッセージを生成
#[allow(dead_code)]
pub fn make_event(method: &str, payload: serde_json::Value) -> ProtocolMessage {
    ProtocolMessage::new_with_json(0, method.to_string(), MessageType::Event, payload).unwrap()
}

/// テスト用 ServerIdentity を構築
#[allow(dead_code)]
pub fn make_identity(name: &str, channels: &[&str]) -> unison::network::identity::ServerIdentity {
    use unison::network::identity::*;
    let mut identity = ServerIdentity::new(name, "0.1.0", "test");
    for ch in channels {
        identity.add_channel(ChannelInfo {
            name: ch.to_string(),
            direction: ChannelDirection::Bidirectional,
            lifetime: "persistent".to_string(),
            status: ChannelStatus::Available,
        });
    }
    identity
}
