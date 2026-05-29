//! Discovery channel handler — server-side handler for `unison.discovery`.
//!
//! 役割: client からの `GetProtocol` request に対して、 server 自身の protocol KDL
//! (= memoize 済 [`ProtocolCache`]) を [`ProtocolDocument`] として返す。
//!
//! 設計: `spec/04-discovery/SPEC.md`
//! KDL: `schemas/discovery.kdl`
//!
//! # 典型使用 (server 側)
//!
//! ```ignore
//! let server = ProtocolServer::new();
//! let kdl = std::fs::read_to_string("schemas/discovery.kdl")?;
//! server.enable_discovery(kdl).await?;
//! // 以降、 client は client.open_channel("unison.discovery") で
//! // GetProtocol request を投げて ProtocolDocument を受け取れる。
//! ```

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::protocol_cache::ProtocolCache;
use super::quic::UnisonStream;
use super::{MessageType, NetworkError, UnisonChannel};

/// `unison.discovery` channel name (= `schemas/discovery.kdl` 側と一致)
pub const DISCOVERY_CHANNEL_NAME: &str = "unison.discovery";

/// `GetProtocol` request method name (= `schemas/discovery.kdl` 側と一致)
pub const GET_PROTOCOL_METHOD: &str = "GetProtocol";

/// `SchemaUpdated` event method name (= v0.1.0 では emit されない、 名前のみ確定)
pub const SCHEMA_UPDATED_EVENT: &str = "SchemaUpdated";

/// `GetProtocol` request payload (client → server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetProtocolRequest {
    /// `"kdl"` (= 生 KDL ソース) または `"kdl+hash"` (= hash 付き、 v0.1.0 では同等)
    pub format: String,
}

/// `ProtocolDocument` response payload (server → client)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolDocument {
    /// KDL ソース全文 (= UTF-8 raw text)
    pub kdl: String,
    /// `protocol "..." version="..."` の version 値
    pub version: String,
    /// kdl 本文の SHA-256 hex (= 64 文字 lowercase)
    pub hash: String,
    /// server が話せる codec 一覧 (= v0.1.0 は `["json"]` 固定)
    pub codecs: Vec<String>,
}

impl ProtocolDocument {
    fn from_cache(cache: &ProtocolCache) -> Self {
        Self {
            kdl: cache.kdl.to_string(),
            version: cache.version.to_string(),
            hash: cache.hash.to_string(),
            codecs: cache.codecs.iter().cloned().collect(),
        }
    }

    /// `kdl` の SHA-256 が `hash` field と一致するか検証する (= 配信 KDL の integrity)。
    ///
    /// client は `unison.discovery` を `"kdl+hash"` で叩くので hash は必須。 server 側
    /// [`ProtocolCache`] と同一の `sha256_hex` で再計算して突き合わせる。
    pub fn verify_integrity(&self) -> bool {
        super::protocol_cache::sha256_hex(self.kdl.as_bytes()) == self.hash
    }
}

/// `SchemaUpdated` event payload (server → client、 v0.1.0 では emit されない)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaUpdatedEvent {
    pub new_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
}

/// `unison.discovery` channel handler loop。
///
/// 1 connection 毎に 1 回起動され、 channel が close するまで request を待ち受ける。
/// `GetProtocol` request に対して `ProtocolDocument` を返す。 他 method / event は
/// debug log を吐いて無視する (= forward-compat、 future request method 追加を破壊しない)。
pub async fn handle_channel(
    cache: Arc<ProtocolCache>,
    stream: UnisonStream,
) -> Result<(), NetworkError> {
    let channel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                if msg.method == GET_PROTOCOL_METHOD {
                    // format は v0.1.0 では未使用 (= 同じ ProtocolDocument を返す、
                    // future hint として field は受理するだけ)。 malformed payload は
                    // default `"kdl"` 扱いで debug log 出すが reject しない (= 寛容)。
                    if msg.payload_as_value().is_err() {
                        tracing::debug!(
                            "discovery: GetProtocol payload non-JSON, treating as default"
                        );
                    }

                    let doc = ProtocolDocument::from_cache(&cache);
                    let payload = serde_json::to_value(&doc)?;
                    channel
                        .send_response(msg.id, GET_PROTOCOL_METHOD, &payload)
                        .await?;

                    tracing::debug!(
                        version = %cache.version,
                        hash = %&cache.hash[..16.min(cache.hash.len())],
                        "discovery: served GetProtocol"
                    );
                } else {
                    tracing::warn!(
                        method = %msg.method,
                        "discovery: unknown request method, ignoring (= forward-compat)"
                    );
                }
            }
            Ok(msg) => {
                tracing::debug!(
                    method = %msg.method,
                    msg_type = ?msg.msg_type,
                    "discovery: ignored non-request"
                );
            }
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// const が schemas/discovery.kdl と一致していることの guard test。
    /// schemas を編集したら必ずこの test も更新する。
    #[test]
    fn discovery_names_match_kdl_schema() {
        assert_eq!(DISCOVERY_CHANNEL_NAME, "unison.discovery");
        assert_eq!(GET_PROTOCOL_METHOD, "GetProtocol");
        assert_eq!(SCHEMA_UPDATED_EVENT, "SchemaUpdated");
    }

    /// ProtocolCache → ProtocolDocument round-trip
    #[test]
    fn protocol_document_from_cache_preserves_fields() {
        let kdl = r#"
protocol "demo" version="0.1.0" {
    namespace "d.ns"
    channel "c" from="client" lifetime="persistent" {
        request "R" { returns "X" {} }
    }
}
"#;
        let cache = ProtocolCache::new(kdl).unwrap();
        let doc = ProtocolDocument::from_cache(&cache);
        assert_eq!(doc.version, "0.1.0");
        assert_eq!(doc.hash.len(), 64);
        assert_eq!(doc.codecs, vec!["json".to_string()]);
        assert!(doc.kdl.contains("protocol \"demo\""));
    }

    /// verify_integrity: cache 由来の document は整合、 hash 改竄 / kdl 改竄は検出
    #[test]
    fn protocol_document_verify_integrity() {
        let kdl = "protocol \"demo\" version=\"0.1.0\" { }";
        let cache = ProtocolCache::new(kdl).unwrap();
        let doc = ProtocolDocument::from_cache(&cache);
        assert!(doc.verify_integrity(), "正規 document は整合すべき");

        // hash を改竄 → 不整合
        let mut tampered_hash = doc.clone();
        tampered_hash.hash = "0".repeat(64);
        assert!(!tampered_hash.verify_integrity());

        // kdl を改竄 (hash 据え置き) → 不整合
        let mut tampered_kdl = doc.clone();
        tampered_kdl.kdl.push_str("\n// injected");
        assert!(!tampered_kdl.verify_integrity());
    }

    /// ProtocolDocument の JSON serde round-trip
    #[test]
    fn protocol_document_serde_round_trip() {
        let doc = ProtocolDocument {
            kdl: "protocol \"x\" version=\"1.0.0\" { }".to_string(),
            version: "1.0.0".to_string(),
            hash: "abc".repeat(21) + "d", // dummy 64 char
            codecs: vec!["json".to_string(), "proto".to_string()],
        };
        let json = serde_json::to_value(&doc).unwrap();
        let restored: ProtocolDocument = serde_json::from_value(json).unwrap();
        assert_eq!(restored.kdl, doc.kdl);
        assert_eq!(restored.version, doc.version);
        assert_eq!(restored.hash, doc.hash);
        assert_eq!(restored.codecs, doc.codecs);
    }

    /// GetProtocolRequest の JSON serde
    #[test]
    fn get_protocol_request_serde() {
        let req = GetProtocolRequest {
            format: "kdl+hash".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"format\""));
        assert!(json.contains("\"kdl+hash\""));
        let restored: GetProtocolRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.format, "kdl+hash");
    }

    /// SchemaUpdatedEvent の new_version は omit 可能
    #[test]
    fn schema_updated_event_omits_optional_version() {
        let evt = SchemaUpdatedEvent {
            new_hash: "h".repeat(64),
            new_version: None,
        };
        let json = serde_json::to_value(&evt).unwrap();
        assert!(
            json.get("new_version").is_none(),
            "new_version should be omitted when None"
        );
        assert!(json.get("new_hash").is_some());
    }
}
