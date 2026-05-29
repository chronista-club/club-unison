//! ProtocolCache — server-side KDL cache for the `unison.discovery` channel.
//!
//! Server が runtime 配信する protocol KDL を 1 度 load して以下を memoize する:
//!
//! - 生 KDL 文字列 (= `ProtocolDocument.kdl` の中身)
//! - version (= KDL の `protocol "..." version="..."` 値)
//! - SHA-256 hex hash (= `ProtocolDocument.hash`、 client cache validation 用)
//! - codecs 一覧 (= v0.1.0 では `["json"]` 固定)
//!
//! 設計: `spec/04-discovery/SPEC.md`

use sha2::{Digest, Sha256};
use std::sync::Arc;

use super::NetworkError;
use crate::parser::SchemaParser;

/// Server が `unison.discovery` channel で配信する protocol KDL を memoize したもの。
///
/// build time に KDL を渡して構築し、 discovery channel handler が `Arc<ProtocolCache>`
/// 経由で参照する (= response 毎に再計算しない)。 v0.1.0 では構築後 immutable、
/// hot reload (= cache 差し替え) は別 Epic (Hailing-α P4 = SchemaUpdated event)。
#[derive(Debug, Clone)]
pub struct ProtocolCache {
    /// KDL ソース全文 (= UTF-8 raw text)
    pub kdl: Arc<str>,
    /// `protocol "..." version="..."` の version 値
    pub version: Arc<str>,
    /// kdl 本文の SHA-256 hex (= 64 文字 lowercase)
    pub hash: Arc<str>,
    /// server が話せる codec 一覧 (= v0.1.0 は `["json"]` 固定)
    pub codecs: Arc<[String]>,
}

impl ProtocolCache {
    /// KDL ソースから `ProtocolCache` を構築する。
    ///
    /// - version は KDL を parse して `protocol "..." version="..."` から自動抽出
    /// - hash は kdl bytes の SHA-256 hex (= byte-exact、 改行 / 空白も含む)
    /// - codecs は `["json"]` 固定
    ///
    /// # Errors
    /// KDL が parse できない or `protocol` block を欠く場合は
    /// [`NetworkError::Protocol`] を返す。
    pub fn new(kdl: impl Into<String>) -> Result<Self, NetworkError> {
        let kdl: String = kdl.into();

        // version 抽出 (= 既存 SchemaParser を流用、 schema-lint と同じ入口)
        let parser = SchemaParser::new();
        let schema = parser
            .parse(&kdl)
            .map_err(|e| NetworkError::Protocol(format!("discovery: KDL parse failed: {e}")))?;
        let protocol = schema.protocol.as_ref().ok_or_else(|| {
            NetworkError::Protocol("discovery: KDL has no `protocol` block".to_string())
        })?;
        let version = protocol.version.clone();

        let hash = sha256_hex(kdl.as_bytes());

        Ok(Self {
            kdl: Arc::from(kdl.as_str()),
            version: Arc::from(version.as_str()),
            hash: Arc::from(hash.as_str()),
            codecs: Arc::from(vec!["json".to_string()]),
        })
    }

    /// ファイルから KDL を読み込んで `ProtocolCache` を構築する。
    ///
    /// # Errors
    /// 読み込み失敗時は [`NetworkError::Connection`]、 KDL 不正時は
    /// [`NetworkError::Protocol`]。
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, NetworkError> {
        let path = path.as_ref();
        let kdl = std::fs::read_to_string(path).map_err(|e| {
            NetworkError::Connection(format!("discovery: failed to read {}: {e}", path.display()))
        })?;
        Self::new(kdl)
    }
}

/// bytes の SHA-256 を 64 文字 lowercase hex で返す。
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex_lower(&digest)
}

/// bytes を lowercase hex 文字列に変換 (= 余分な dep 回避の独自実装)。
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 の known answer (= empty + "abc") で hex 出力を検証
    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// valid KDL から version / hash / codecs / kdl を構築できる
    #[test]
    fn cache_extracts_version_and_hashes_kdl() {
        let kdl = r#"
protocol "demo" version="1.2.3" {
    namespace "demo.ns"
    channel "x" from="client" lifetime="persistent" {
        request "Y" {
            returns "Z" {
                field "ok" type="bool"
            }
        }
    }
}
"#;
        let cache = ProtocolCache::new(kdl).unwrap();
        assert_eq!(&*cache.version, "1.2.3");
        assert_eq!(cache.hash.len(), 64);
        assert!(
            cache
                .hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hash must be lowercase hex: {}",
            cache.hash
        );
        assert!(cache.kdl.contains("protocol \"demo\""));
        assert_eq!(&cache.codecs[..], &["json".to_string()][..]);
    }

    /// 同じ KDL からは決定的に同じ hash が出る (= cache validation 用に必須)
    #[test]
    fn cache_hash_is_deterministic() {
        let kdl = r#"protocol "x" version="0.1.0" { namespace "a" channel "c" from="client" lifetime="persistent" { request "R" { returns "X" {} } } }"#;
        let a = ProtocolCache::new(kdl).unwrap();
        let b = ProtocolCache::new(kdl).unwrap();
        assert_eq!(a.hash, b.hash);
    }

    /// malformed KDL は Err
    #[test]
    fn cache_rejects_malformed_kdl() {
        let bad = "this is not valid kdl }{}{)(";
        assert!(ProtocolCache::new(bad).is_err());
    }

    /// ファイル read OK → KDL parse → cache 構築
    #[test]
    fn cache_from_file_reads_and_parses() {
        // 既存 schemas/ping_pong.kdl を使う (= 小さくて読みやすい)
        let path = std::env::var("CARGO_MANIFEST_DIR")
            .map(|d| std::path::PathBuf::from(d).join("../../schemas/ping_pong.kdl"))
            .unwrap_or_else(|_| std::path::PathBuf::from("../../schemas/ping_pong.kdl"));
        let cache = ProtocolCache::from_file(&path).expect("read + parse");
        assert_eq!(&*cache.version, "2.0.0");
        assert!(cache.kdl.contains("ping-pong"));
    }

    /// 存在しないファイルは Connection error
    #[test]
    fn cache_from_file_missing_returns_connection_error() {
        let result = ProtocolCache::from_file("/nonexistent/path/to/file.kdl");
        match result {
            Err(NetworkError::Connection(msg)) => {
                assert!(msg.contains("failed to read"), "got: {msg}");
            }
            other => panic!("expected Connection error, got: {other:?}"),
        }
    }
}
