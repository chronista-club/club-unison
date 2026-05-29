//! DynamicProtocol — client-side wrapper that discovers a server's protocol
//! schema at runtime and exposes type-validated channels.
//!
//! Server が `unison.discovery` channel で配信する KDL を fetch して
//! [`SchemaRegistry`] を build し、 以降の `open_channel` / `DynamicChannel::request`
//! が registry に対して payload validation を実行する。
//!
//! 設計: `spec/04-discovery/SPEC.md` §8 (= Client 側 TypeRegistry 構築)、
//!       Unison Hailing α Epic P2-Rust。
//!
//! # 典型使用 (client 側)
//!
//! ```ignore
//! use std::sync::Arc;
//! use unison::network::{DynamicProtocol, ProtocolClient};
//!
//! let client = Arc::new(ProtocolClient::new_default()?);
//! client.connect("[::1]:7878").await?;
//! let proto = DynamicProtocol::fetch(client.clone()).await?;
//!
//! // registry を inspect
//! println!("protocol: {} v{}", proto.protocol_name(), proto.version());
//! for ch in proto.registry().channels() {
//!     println!("  channel: {}", ch.name);
//! }
//!
//! // typed channel call (= validation 経由)
//! let chan = proto.open_channel("memory.search").await?;
//! let resp = chan.request("Search", json!({"query": "..."})).await?;
//! ```

use std::sync::Arc;
use thiserror::Error;

use super::discovery::{DISCOVERY_CHANNEL_NAME, GET_PROTOCOL_METHOD, ProtocolDocument};
use super::schema_registry::{RegistryError, SchemaRegistry, ValidationError};
use super::{NetworkError, ProtocolClient, UnisonChannel};

/// Client-side dynamic protocol — fetched at runtime, exposes typed channels.
pub struct DynamicProtocol {
    client: Arc<ProtocolClient>,
    registry: Arc<SchemaRegistry>,
    document: ProtocolDocument,
}

/// `DynamicProtocol` の操作で出る複合エラー (= network + validation)
#[derive(Debug, Error)]
pub enum DynamicError {
    #[error("network error: {0}")]
    Network(#[from] NetworkError),

    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("registry build failed: {0}")]
    Registry(#[from] RegistryError),

    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("integrity check failed: kdl の SHA-256 が advertised hash ({expected}) と不一致")]
    Integrity { expected: String },
}

impl DynamicProtocol {
    /// 既に接続済 [`ProtocolClient`] から `unison.discovery` を叩いて
    /// `ProtocolDocument` を fetch し、 `SchemaRegistry` を build して返す。
    ///
    /// # Errors
    /// - discovery channel が server で有効化されていない → `NetworkError::HandlerNotFound`
    /// - KDL が parse できない → `DynamicError::Registry`
    /// - 通信失敗 → `DynamicError::Network`
    pub async fn fetch(client: Arc<ProtocolClient>) -> Result<Self, DynamicError> {
        let channel = client.open_channel(DISCOVERY_CHANNEL_NAME).await?;
        let value: serde_json::Value = channel
            .request(
                GET_PROTOCOL_METHOD,
                &serde_json::json!({ "format": "kdl+hash" }),
            )
            .await?;
        // discovery channel は単発の request だけ使う、 以降 SchemaUpdated event は
        // hot reload Epic で扱うため、 ここでは close する。
        let _ = channel.close().await;

        let document: ProtocolDocument = serde_json::from_value(value)?;
        // 配信 KDL の integrity を検証する (= client は "kdl+hash" を要求しているので
        // hash は必須)。 hash field を信頼した hot-reload 差分判定 (SchemaUpdated) の
        // 前提でもあり、 改竄 / 破損の早期検出になる。
        if !document.verify_integrity() {
            return Err(DynamicError::Integrity {
                expected: document.hash.to_string(),
            });
        }
        let registry = SchemaRegistry::from_kdl(&document.kdl)?;

        Ok(Self {
            client,
            registry: Arc::new(registry),
            document,
        })
    }

    /// 取得済の `ProtocolDocument` を参照する
    pub fn document(&self) -> &ProtocolDocument {
        &self.document
    }

    /// schema registry を参照
    pub fn registry(&self) -> &SchemaRegistry {
        &self.registry
    }

    /// protocol name (= `protocol "<name>" version="<v>"` の `<name>`)
    pub fn protocol_name(&self) -> &str {
        self.registry.protocol_name()
    }

    /// protocol version (= ProtocolDocument.version と同値、 KDL から抽出)
    pub fn version(&self) -> &str {
        self.registry.protocol_version()
    }

    /// 配信 KDL の SHA-256 hex (= ProtocolDocument.hash)
    pub fn hash(&self) -> &str {
        &self.document.hash
    }

    /// server が話せる codec 一覧 (= v0.1.0 は ["json"] 固定)
    pub fn codecs(&self) -> &[String] {
        &self.document.codecs
    }

    /// schema registry に基づく typed channel を open する。
    ///
    /// channel name が registry に存在しない場合は
    /// `DynamicError::Validation(ChannelNotFound)` を返し、 server には接続しない
    /// (= fail-fast)。
    pub async fn open_channel(&self, name: &str) -> Result<DynamicChannel, DynamicError> {
        if self.registry.channel(name).is_none() {
            return Err(DynamicError::Validation(ValidationError::ChannelNotFound(
                name.to_string(),
            )));
        }
        let inner = self.client.open_channel(name).await?;
        Ok(DynamicChannel {
            inner,
            registry: Arc::clone(&self.registry),
            channel_name: name.to_string(),
        })
    }
}

/// Validation-aware channel handle。
///
/// `request` 前に [`SchemaRegistry::validate_request`] を呼び、 schema mismatch を
/// **server には送らずに** error として返す (= fail-fast、 network cost を節約)。
pub struct DynamicChannel {
    inner: UnisonChannel,
    registry: Arc<SchemaRegistry>,
    channel_name: String,
}

impl DynamicChannel {
    /// 自分が属する channel の名前
    pub fn channel_name(&self) -> &str {
        &self.channel_name
    }

    /// 内側の生 `UnisonChannel` への参照 (= escape hatch、 validation を回避したい時用)
    pub fn inner(&self) -> &UnisonChannel {
        &self.inner
    }

    /// payload を schema に対して validate し、 OK なら underlying channel へ request 送信
    pub async fn request(
        &self,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, DynamicError> {
        self.registry
            .validate_request(&self.channel_name, method, &payload)?;
        let resp: serde_json::Value = self.inner.request(method, &payload).await?;
        Ok(resp)
    }

    /// channel を閉じる
    pub async fn close(self) -> Result<(), NetworkError> {
        self.inner.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DynamicError variants の `From` impl が機能する
    #[test]
    fn dynamic_error_from_conversions() {
        let net: DynamicError = NetworkError::NotConnected.into();
        assert!(matches!(net, DynamicError::Network(_)));

        let val: DynamicError = ValidationError::ChannelNotFound("x".to_string()).into();
        assert!(matches!(val, DynamicError::Validation(_)));
    }
}
