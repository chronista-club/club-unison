//! SchemaRegistry — runtime channel schema registry built from a KDL protocol document.
//!
//! `DynamicProtocol` ([`super::dynamic`]) が `unison.discovery` 経由で fetch した
//! KDL を parse して構築する。 channel → request/event の型情報を保持し、 client が
//! request を送る前に payload validation を実行できるようにする。
//!
//! 設計: `spec/04-discovery/SPEC.md` §8 (= client 側 TypeRegistry 構築)。
//!
//! # 役割
//! - channel / request / event の **存在** 検証
//! - request payload の `required` field 欠落検知
//! - request payload の field 型 (JSON 基本型) 一致検知
//!
//! v0.1.0 では constraint (min/max/pattern) や response 側 validation は **out of
//! scope** (= 別 Epic / phase)。 `FieldType::Custom`、 `FieldType::Enum` も
//! v0.1.0 では type check を passthrough する (= 後続 phase で本格対応)。
//!
//! # 既存 `parser::TypeRegistry` との違い
//! `parser::TypeRegistry` は **build-time codegen** 用の型名 mapping (= KDL の
//! `typedef` 名から Rust / TypeScript の型文字列を引く)。 本 `SchemaRegistry` は
//! **runtime channel schema lookup** + validation。 役割が直交するため別 type。

use std::collections::HashMap;
use thiserror::Error;

use crate::parser::{Channel, ChannelRequest, Field, FieldType, ParsedSchema, SchemaParser};

/// Runtime channel schema registry。
///
/// `from_kdl` で KDL ソースを parse して構築する。 構築後は immutable、 hot reload
/// (= 別 registry を build して swap) は呼び出し側責務。
#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    schema: ParsedSchema,
    /// channel name → index into `schema.protocol.channels`
    channel_index: HashMap<String, usize>,
}

/// SchemaRegistry 構築時のエラー
///
/// Parse は upstream `SchemaParser` が anyhow を返すため、 caller-facing なエラー
/// は文字列化 (= boundary で error type を簡略化、 caller に anyhow を leak しない)。
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("KDL parse failed: {0}")]
    Parse(String),

    #[error("KDL has no `protocol` block")]
    NoProtocol,
}

/// Validation error (= caller が registry に対して request を送る前の事前検証)
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("channel '{0}' not found in registry")]
    ChannelNotFound(String),

    #[error("method '{method}' not found in channel '{channel}'")]
    MethodNotFound { channel: String, method: String },

    #[error(
        "request payload must be a JSON object for {channel}.{method}, got {got}"
    )]
    PayloadNotObject {
        channel: String,
        method: String,
        got: String,
    },

    #[error(
        "required field '{field}' missing in request {channel}.{method}"
    )]
    MissingRequired {
        channel: String,
        method: String,
        field: String,
    },

    #[error(
        "type mismatch for {channel}.{method}.{field}: expected {expected}, got {got}"
    )]
    TypeMismatch {
        channel: String,
        method: String,
        field: String,
        expected: String,
        got: String,
    },
}

impl SchemaRegistry {
    /// KDL ソースから SchemaRegistry を構築する。
    ///
    /// 既存 [`SchemaParser`] を使用。 `protocol` block を欠く KDL は Err。
    pub fn from_kdl(kdl: &str) -> Result<Self, RegistryError> {
        let parser = SchemaParser::new();
        let schema = parser
            .parse(kdl)
            .map_err(|e| RegistryError::Parse(e.to_string()))?;
        if schema.protocol.is_none() {
            return Err(RegistryError::NoProtocol);
        }
        let channel_index = schema
            .protocol
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect();
        Ok(Self {
            schema,
            channel_index,
        })
    }

    fn protocol(&self) -> &crate::parser::Protocol {
        // from_kdl で None なら Err にしているので unwrap 安全
        self.schema.protocol.as_ref().expect("protocol present (verified in from_kdl)")
    }

    /// `protocol "<name>" version="<version>"` の name 部分
    pub fn protocol_name(&self) -> &str {
        &self.protocol().name
    }

    /// `protocol "<name>" version="<version>"` の version 部分
    pub fn protocol_version(&self) -> &str {
        &self.protocol().version
    }

    /// `namespace "<value>"` の値 (= 未指定なら空文字列)
    pub fn protocol_namespace(&self) -> &str {
        self.protocol().namespace.as_deref().unwrap_or("")
    }

    /// 全 channel descriptor を iterate (= 順序は KDL 定義順)
    pub fn channels(&self) -> impl Iterator<Item = &Channel> {
        self.protocol().channels.iter()
    }

    /// 名前で channel descriptor を引く
    pub fn channel(&self, name: &str) -> Option<&Channel> {
        let idx = *self.channel_index.get(name)?;
        self.protocol().channels.get(idx)
    }

    /// channel.method の request descriptor を引く
    pub fn request(&self, channel_name: &str, method: &str) -> Option<&ChannelRequest> {
        self.channel(channel_name)?
            .requests
            .iter()
            .find(|r| r.name == method)
    }

    /// request payload を schema に対して validate する。
    ///
    /// 検証項目:
    /// 1. channel が存在する
    /// 2. method (= request name) が channel に存在する
    /// 3. payload が JSON object
    /// 4. `required: true` の field が全て payload に含まれる
    /// 5. 各 field の JSON 型が KDL field type と一致する (= 基本型のみ、 `Custom`
    ///    `Enum` は passthrough)
    ///
    /// payload に schema 外の追加 field があっても許容する (= forward-compat、
    /// schema evolution で field 追加が non-breaking)。
    pub fn validate_request(
        &self,
        channel_name: &str,
        method: &str,
        payload: &serde_json::Value,
    ) -> Result<(), ValidationError> {
        let channel = self
            .channel(channel_name)
            .ok_or_else(|| ValidationError::ChannelNotFound(channel_name.to_string()))?;
        let request = channel
            .requests
            .iter()
            .find(|r| r.name == method)
            .ok_or_else(|| ValidationError::MethodNotFound {
                channel: channel_name.to_string(),
                method: method.to_string(),
            })?;

        let obj = payload
            .as_object()
            .ok_or_else(|| ValidationError::PayloadNotObject {
                channel: channel_name.to_string(),
                method: method.to_string(),
                got: value_type_name(payload).to_string(),
            })?;

        for field in &request.fields {
            validate_field(channel_name, method, field, obj.get(&field.name))?;
        }
        Ok(())
    }
}

/// 単一 field の validation
fn validate_field(
    channel: &str,
    method: &str,
    field: &Field,
    value: Option<&serde_json::Value>,
) -> Result<(), ValidationError> {
    match value {
        None => {
            if field.required {
                Err(ValidationError::MissingRequired {
                    channel: channel.to_string(),
                    method: method.to_string(),
                    field: field.name.clone(),
                })
            } else {
                Ok(())
            }
        }
        Some(v) => {
            // null は optional 値の 「未指定」 として許容 (= JSON convention)
            if v.is_null() {
                return Ok(());
            }
            let expected = field.field_type();
            let ok = type_matches(&expected, v);
            if ok {
                Ok(())
            } else {
                Err(ValidationError::TypeMismatch {
                    channel: channel.to_string(),
                    method: method.to_string(),
                    field: field.name.clone(),
                    expected: field_type_name(&expected).to_string(),
                    got: value_type_name(v).to_string(),
                })
            }
        }
    }
}

/// JSON 値が KDL field type に matches するか (= basic JSON type level check のみ)
fn type_matches(expected: &FieldType, value: &serde_json::Value) -> bool {
    match expected {
        FieldType::String => value.is_string(),
        FieldType::Int => value.is_i64() || value.is_u64(),
        FieldType::Float => value.is_f64() || value.is_i64() || value.is_u64(),
        FieldType::Bool => value.is_boolean(),
        FieldType::Json => true,                      // 任意 JSON OK
        FieldType::Object => value.is_object(),
        FieldType::Array(_) => value.is_array(),     // 要素型まで深掘りしない (v0.1.0)
        FieldType::Map(_, _) => value.is_object(),
        FieldType::Custom(_) => true,                // 未知型は passthrough
        FieldType::Enum(_) => true,                  // enum も v0.1.0 は passthrough
    }
}

/// FieldType の人間可読な名前
fn field_type_name(t: &FieldType) -> &'static str {
    match t {
        FieldType::String => "string",
        FieldType::Int => "int",
        FieldType::Float => "float",
        FieldType::Bool => "bool",
        FieldType::Json => "json",
        FieldType::Object => "object",
        FieldType::Array(_) => "array",
        FieldType::Map(_, _) => "map",
        FieldType::Custom(_) => "custom",
        FieldType::Enum(_) => "enum",
    }
}

/// serde_json::Value の人間可読な型名
fn value_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "int"
            } else {
                "float"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SAMPLE_KDL: &str = r#"
protocol "demo" version="1.0.0" {
    namespace "demo.ns"

    channel "chat" from="client" lifetime="persistent" {
        request "Send" {
            field "to" type="string" required=#true
            field "msg" type="string" required=#true
            field "reply_to" type="string"
            returns "Ack" {
                field "ok" type="bool" required=#true
            }
        }
        event "Received" {
            field "from" type="string" required=#true
        }
    }

    channel "metrics" from="server" lifetime="persistent" {
        request "Query" {
            field "limit" type="int"
            field "tags" type="json"
            returns "Result" {
                field "data" type="json"
            }
        }
    }
}
"#;

    fn registry() -> SchemaRegistry {
        SchemaRegistry::from_kdl(SAMPLE_KDL).unwrap()
    }

    #[test]
    fn from_kdl_extracts_protocol_metadata() {
        let r = registry();
        assert_eq!(r.protocol_name(), "demo");
        assert_eq!(r.protocol_version(), "1.0.0");
        assert_eq!(r.protocol_namespace(), "demo.ns");
    }

    #[test]
    fn channel_lookup_works_for_known_and_unknown() {
        let r = registry();
        assert!(r.channel("chat").is_some());
        assert!(r.channel("metrics").is_some());
        assert!(r.channel("ghost").is_none());
    }

    #[test]
    fn request_lookup_by_channel_and_method() {
        let r = registry();
        assert!(r.request("chat", "Send").is_some());
        assert!(r.request("chat", "Unknown").is_none());
        assert!(r.request("ghost", "Send").is_none());
        let req = r.request("metrics", "Query").unwrap();
        assert_eq!(req.name, "Query");
        assert!(req.returns.is_some());
    }

    #[test]
    fn channels_iterator_preserves_order() {
        let r = registry();
        let names: Vec<&str> = r.channels().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["chat", "metrics"]);
    }

    #[test]
    fn validate_accepts_valid_payload() {
        let r = registry();
        let ok = r.validate_request(
            "chat",
            "Send",
            &json!({"to": "alice", "msg": "hi"}),
        );
        assert!(ok.is_ok(), "got: {:?}", ok);
    }

    #[test]
    fn validate_accepts_optional_field_omitted() {
        let r = registry();
        let ok = r.validate_request(
            "chat",
            "Send",
            &json!({"to": "alice", "msg": "hi"}),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_accepts_optional_field_present() {
        let r = registry();
        let ok = r.validate_request(
            "chat",
            "Send",
            &json!({"to": "alice", "msg": "hi", "reply_to": "thread-1"}),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_accepts_null_for_optional_field() {
        let r = registry();
        let ok = r.validate_request(
            "chat",
            "Send",
            &json!({"to": "alice", "msg": "hi", "reply_to": null}),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_accepts_extra_fields() {
        // forward-compat: schema 外の field は許容
        let r = registry();
        let ok = r.validate_request(
            "chat",
            "Send",
            &json!({"to": "alice", "msg": "hi", "future_field": 42}),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_rejects_unknown_channel() {
        let r = registry();
        let err = r
            .validate_request("ghost", "X", &json!({}))
            .unwrap_err();
        assert_eq!(err, ValidationError::ChannelNotFound("ghost".to_string()));
    }

    #[test]
    fn validate_rejects_unknown_method() {
        let r = registry();
        let err = r
            .validate_request("chat", "Unknown", &json!({}))
            .unwrap_err();
        assert_eq!(
            err,
            ValidationError::MethodNotFound {
                channel: "chat".to_string(),
                method: "Unknown".to_string()
            }
        );
    }

    #[test]
    fn validate_rejects_non_object_payload() {
        let r = registry();
        let err = r
            .validate_request("chat", "Send", &json!("not an object"))
            .unwrap_err();
        match err {
            ValidationError::PayloadNotObject { got, .. } => assert_eq!(got, "string"),
            other => panic!("expected PayloadNotObject, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_missing_required() {
        let r = registry();
        let err = r
            .validate_request("chat", "Send", &json!({"to": "alice"}))
            .unwrap_err();
        assert_eq!(
            err,
            ValidationError::MissingRequired {
                channel: "chat".to_string(),
                method: "Send".to_string(),
                field: "msg".to_string()
            }
        );
    }

    #[test]
    fn validate_rejects_wrong_type_string() {
        let r = registry();
        // to は string、 int を渡すと TypeMismatch
        let err = r
            .validate_request("chat", "Send", &json!({"to": 42, "msg": "hi"}))
            .unwrap_err();
        match err {
            ValidationError::TypeMismatch {
                field, expected, got, ..
            } => {
                assert_eq!(field, "to");
                assert_eq!(expected, "string");
                assert_eq!(got, "int");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_int_for_int_field() {
        let r = registry();
        let ok = r.validate_request("metrics", "Query", &json!({"limit": 10}));
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_rejects_string_for_int_field() {
        let r = registry();
        let err = r
            .validate_request("metrics", "Query", &json!({"limit": "ten"}))
            .unwrap_err();
        match err {
            ValidationError::TypeMismatch { expected, got, .. } => {
                assert_eq!(expected, "int");
                assert_eq!(got, "string");
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_anything_for_json_field() {
        let r = registry();
        // tags is `type="json"` → 何でも OK
        assert!(r
            .validate_request("metrics", "Query", &json!({"tags": ["a", "b"]}))
            .is_ok());
        assert!(r
            .validate_request("metrics", "Query", &json!({"tags": "string-also-ok"}))
            .is_ok());
        assert!(r
            .validate_request("metrics", "Query", &json!({"tags": 42}))
            .is_ok());
    }

    #[test]
    fn from_kdl_rejects_no_protocol_block() {
        // 完全に protocol を欠く KDL
        let result = SchemaRegistry::from_kdl("");
        // parser によっては empty も通すかもしれない。 protocol が無ければ Err
        assert!(matches!(result, Err(RegistryError::NoProtocol)) || result.is_err());
    }

    #[test]
    fn from_kdl_rejects_malformed() {
        let result = SchemaRegistry::from_kdl("not valid kdl }{");
        assert!(result.is_err());
    }
}
