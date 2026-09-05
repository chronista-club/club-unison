//! Identity Channel: QUICエンドポイントのリアルタイム自己紹介
//!
//! 各サーバーは接続時にServerIdentityを送信し、
//! チャネルの追加・削除・状態変更をリアルタイムに通知する。

use serde::{Deserialize, Serialize};

use super::{MessageType, ProtocolMessage};

/// サーバーの自己紹介情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub namespace: String,
    pub channels: Vec<ChannelInfo>,
    pub metadata: serde_json::Value,
}

/// チャネルの情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub name: String,
    pub direction: ChannelDirection,
    pub lifetime: String,
    pub status: ChannelStatus,
}

/// チャネルの方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDirection {
    ServerToClient,
    ClientToServer,
    Bidirectional,
}

/// チャネルの状態
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStatus {
    Available,
    Busy,
    Unavailable,
}

impl ServerIdentity {
    /// 新しいIdentityを作成
    pub fn new(name: &str, version: &str, namespace: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            namespace: namespace.to_string(),
            channels: Vec::new(),
            metadata: serde_json::Value::Null,
        }
    }

    /// チャネル情報を追加
    pub fn add_channel(&mut self, channel: ChannelInfo) {
        self.channels.push(channel);
    }

    /// ProtocolMessageに変換（Identity Channel送信用）
    pub fn to_protocol_message(&self) -> ProtocolMessage {
        ProtocolMessage {
            id: 0,
            method: "__identity".to_string(),
            msg_type: MessageType::Event,
            payload: serde_json::to_vec(self).unwrap(),
        }
    }

    /// ProtocolMessageから復元
    pub fn from_protocol_message(msg: &ProtocolMessage) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(&msg.payload)
    }
}
