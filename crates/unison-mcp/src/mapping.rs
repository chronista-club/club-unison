//! KDL channel.request → MCP `Tool` synthesis + KDL field type → JSON Schema converter.
//!
//! 役割:
//! 1. `synthesize_tool(channel_name, request)` で `Tool` 構造体を組み立てる
//! 2. KDL `FieldType` → JSON Schema Draft 7+ object に変換
//! 3. tool name の双方向変換 (= `unison_<channel_safe>_<method>` ↔ (channel, method))
//!
//! KDL → JSON Schema 対応表は `docs/kdl-to-json-schema.md` 参照。
//!
//! # Dual consumer 設計 (Hailing-δ leverage 1)
//!
//! 同じ converter が以下 3 つの consumer に流せる (= 同一 JSON Schema 出力):
//! - MCP `Tool.input_schema` (= 本 P3b の primary use)
//! - Anthropic Messages API `tools[].input_schema` (= structured tool use)
//! - OpenAI `response_format` / Vercel AI SDK `generateObject` / Instructor
//!
//! 全 ecosystem が JSON Schema を共通通貨にしているため、 converter 1 つで全部に届く。

use rmcp::model::{JsonObject, Tool};
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::sync::Arc;

use unison::parser::{ChannelRequest, Field, FieldType};

/// MCP tool name の prefix (= 静的 escape hatch tool と衝突回避)
pub const SYNTH_TOOL_PREFIX: &str = "unison_";

/// tool 名 component の最大長 (= remote 由来の異常に長い名前を切り詰める)
const MAX_NAME_COMPONENT: usize = 48;

/// channel name と method から MCP tool name を組み立てる。
///
/// 規約: `unison_<channel_safe>_<method_safe>`。 channel / method はいずれも
/// remote KDL (= 信頼できない discovery server) 由来なので [`sanitize_name_component`]
/// で `[A-Za-z0-9_]` に正規化してから組み立てる (= 不正文字による tool 名汚染 /
/// 表示崩れ / injection を防ぐ)。
///
/// # Example
///
/// ```ignore
/// assert_eq!(synth_tool_name("chat", "Send"), "unison_chat_Send");
/// assert_eq!(synth_tool_name("unison.discovery", "GetProtocol"), "unison_unison_discovery_GetProtocol");
/// ```
pub fn synth_tool_name(channel: &str, method: &str) -> String {
    let safe_channel = sanitize_name_component(channel);
    let safe_method = sanitize_name_component(method);
    format!("{SYNTH_TOOL_PREFIX}{safe_channel}_{safe_method}")
}

/// tool 名 component を `[A-Za-z0-9_]` に正規化し、 長さを [`MAX_NAME_COMPONENT`] で
/// 切り詰める。 remote 由来の任意文字列 (= 空白 / 制御文字 / 記号 / 超長文字列) が
/// MCP tool 名にそのまま流れ込むのを防ぐ。 空になった場合は `_` を 1 個返す。
fn sanitize_name_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME_COMPONENT)
        .collect();
    if out.is_empty() { "_".to_string() } else { out }
}

/// tool description の最大長 (= remote 由来の巨大 description による tool-list 肥大を防ぐ)
const MAX_DESCRIPTION_LEN: usize = 1024;

/// remote KDL 由来の description を sanitize する。
///
/// 制御文字 (= newline / tab を除く) を空白に置換し、 長さを [`MAX_DESCRIPTION_LEN`]
/// で切り詰める。 description は LLM が読む untrusted な server 提供文字列なので、
/// terminal/format injection と資源消費を抑える (= 自然言語レベルの prompt injection
/// 自体は surface する性質上残るため、 LLM 側で server 提供データとして扱う前提)。
fn sanitize_description(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .take(MAX_DESCRIPTION_LEN)
        .collect()
}

/// `ChannelRequest` を MCP `Tool` に変換する。
///
/// - name: `synth_tool_name(channel, request.name)`
/// - description: KDL `request "X" description="..."` があればその文字列を採用、
///   無ければ formulaic な default (= 「Invoke the X request on the Y channel」) を生成
/// - input_schema: `request.fields` を JSON Schema object に変換
///
/// description は LLM の tool-selection accuracy に強相関するので、 schema 設計時に
/// `description="..."` を書くことで LLM が tool を正しく選びやすくなる。
pub fn synthesize_tool(channel_name: &str, request: &ChannelRequest) -> Tool {
    let name = synth_tool_name(channel_name, &request.name);
    let description = request
        .description
        .as_deref()
        .map(sanitize_description)
        .unwrap_or_else(|| {
            format!(
                "Invoke the `{}` request on the `{}` Unison channel (= runtime-synthesized from server's KDL schema).",
                request.name, channel_name
            )
        });
    let input_schema = fields_to_input_schema(&request.fields);
    Tool::new(
        Cow::Owned(name),
        Cow::Owned(description),
        Arc::new(input_schema),
    )
}

/// `Field` 群から JSON Schema object (= top-level `{type: "object", properties: {...}, required: [...]}`) を組み立てる
pub fn fields_to_input_schema(fields: &[Field]) -> JsonObject {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for f in fields {
        let mut field_schema = field_type_to_schema(&f.field_type());
        if let Some(desc) = f.description.clone() {
            field_schema.insert("description".to_string(), Value::String(desc));
        }
        properties.insert(f.name.clone(), Value::Object(field_schema));
        if f.required {
            required.push(Value::String(f.name.clone()));
        }
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }
    // additionalProperties true (= forward-compat、 schema 外 field 許容)
    schema.insert("additionalProperties".to_string(), Value::Bool(true));
    schema
}

/// 単一 `FieldType` を JSON Schema object に変換する (= `{"type": ..., ...}`)
///
/// KDL → JSON Schema 対応:
/// - `String` → `{"type": "string"}`
/// - `Int` → `{"type": "integer"}`
/// - `Float` → `{"type": "number"}`
/// - `Bool` → `{"type": "boolean"}`
/// - `Json` → `{}` (= 任意 = no type constraint)
/// - `Object` → `{"type": "object"}`
/// - `Array(inner)` → `{"type": "array", "items": <inner schema>}`
/// - `Map(_, v)` → `{"type": "object", "additionalProperties": <v schema>}`
/// - `Custom(_)` / `Enum(_)` → `{}` (= passthrough、 v0.1.0 は型不問)
pub fn field_type_to_schema(ft: &FieldType) -> JsonObject {
    let mut obj = Map::new();
    match ft {
        FieldType::String => {
            obj.insert("type".to_string(), json!("string"));
        }
        FieldType::Int => {
            obj.insert("type".to_string(), json!("integer"));
        }
        FieldType::Float => {
            obj.insert("type".to_string(), json!("number"));
        }
        FieldType::Bool => {
            obj.insert("type".to_string(), json!("boolean"));
        }
        FieldType::Json => {
            // 任意 = no constraint (= empty schema は 「anything OK」)
        }
        FieldType::Object => {
            obj.insert("type".to_string(), json!("object"));
        }
        FieldType::Array(inner) => {
            obj.insert("type".to_string(), json!("array"));
            let inner_schema = field_type_to_schema(inner);
            obj.insert("items".to_string(), Value::Object(inner_schema));
        }
        FieldType::Map(_k, v) => {
            obj.insert("type".to_string(), json!("object"));
            let v_schema = field_type_to_schema(v);
            obj.insert("additionalProperties".to_string(), Value::Object(v_schema));
        }
        FieldType::Custom(_) | FieldType::Enum(_) => {
            // passthrough (= 任意)、 v0.1.0 では type 制約なし。 将来 typedef 解決で具体化
        }
    }
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_tool_name_simple_channel() {
        assert_eq!(synth_tool_name("chat", "Send"), "unison_chat_Send");
    }

    #[test]
    fn synth_tool_name_dotted_channel() {
        assert_eq!(
            synth_tool_name("unison.discovery", "GetProtocol"),
            "unison_unison_discovery_GetProtocol"
        );
    }

    #[test]
    fn synth_tool_name_dashed_channel() {
        assert_eq!(
            synth_tool_name("ping-pong", "Ping"),
            "unison_ping_pong_Ping"
        );
    }

    #[test]
    fn synth_tool_name_sanitizes_hostile_chars() {
        // remote KDL が空白 / 記号 / 制御文字を仕込んでも [A-Za-z0-9_] に潰れる
        let name = synth_tool_name("ev il chan", "drop;tables");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "tool name must be [A-Za-z0-9_] only: {name}"
        );
        assert!(name.starts_with("unison_"));
    }

    #[test]
    fn synth_tool_name_truncates_overlong_component() {
        let long = "a".repeat(500);
        let name = synth_tool_name(&long, &long);
        // prefix + 2 components (各 <= MAX_NAME_COMPONENT) + 区切り `_`
        assert!(name.len() <= "unison_".len() + MAX_NAME_COMPONENT * 2 + 1);
    }

    #[test]
    fn synth_tool_name_empty_component_falls_back() {
        // sanitize 後に空になっても `_` で埋まり、 空 component で壊れない
        // ("unison_" prefix) + ("_" channel) + ("_" 区切り) + ("_" method)
        let name = synth_tool_name("", "");
        assert_eq!(name, "unison____");
    }

    #[test]
    fn sanitize_description_strips_control_and_caps_len() {
        let dirty = format!("hello\x07\x1bworld{}", "x".repeat(2000));
        let clean = sanitize_description(&dirty);
        assert!(!clean.contains('\x07'));
        assert!(!clean.contains('\x1b'));
        assert!(clean.len() <= MAX_DESCRIPTION_LEN);
        // newline / tab は保持
        assert_eq!(sanitize_description("a\nb\tc"), "a\nb\tc");
    }

    #[test]
    fn synthesize_tool_sanitizes_server_description() {
        let req = ChannelRequest {
            name: "Send".to_string(),
            description: Some("evil\x1b[2Jclear".to_string()),
            fields: vec![],
            returns: None,
        };
        let tool = synthesize_tool("chat", &req);
        assert!(!tool.description.as_deref().unwrap_or("").contains('\x1b'));
    }

    #[test]
    fn field_type_string_to_schema() {
        let schema = field_type_to_schema(&FieldType::String);
        assert_eq!(schema.get("type"), Some(&json!("string")));
    }

    #[test]
    fn field_type_int_to_schema_uses_integer() {
        let schema = field_type_to_schema(&FieldType::Int);
        assert_eq!(schema.get("type"), Some(&json!("integer")));
    }

    #[test]
    fn field_type_float_to_schema_uses_number() {
        let schema = field_type_to_schema(&FieldType::Float);
        assert_eq!(schema.get("type"), Some(&json!("number")));
    }

    #[test]
    fn field_type_bool_to_schema() {
        let schema = field_type_to_schema(&FieldType::Bool);
        assert_eq!(schema.get("type"), Some(&json!("boolean")));
    }

    #[test]
    fn field_type_json_to_schema_is_empty() {
        let schema = field_type_to_schema(&FieldType::Json);
        // 任意 = empty schema
        assert!(schema.is_empty());
    }

    #[test]
    fn field_type_object_to_schema() {
        let schema = field_type_to_schema(&FieldType::Object);
        assert_eq!(schema.get("type"), Some(&json!("object")));
    }

    #[test]
    fn field_type_array_to_schema_with_items() {
        let schema = field_type_to_schema(&FieldType::Array(Box::new(FieldType::String)));
        assert_eq!(schema.get("type"), Some(&json!("array")));
        let items = schema.get("items").and_then(Value::as_object).unwrap();
        assert_eq!(items.get("type"), Some(&json!("string")));
    }

    #[test]
    fn field_type_map_to_schema_with_additional_properties() {
        let schema = field_type_to_schema(&FieldType::Map(
            Box::new(FieldType::String),
            Box::new(FieldType::Int),
        ));
        assert_eq!(schema.get("type"), Some(&json!("object")));
        let ap = schema
            .get("additionalProperties")
            .and_then(Value::as_object)
            .unwrap();
        assert_eq!(ap.get("type"), Some(&json!("integer")));
    }

    #[test]
    fn field_type_custom_to_schema_is_empty() {
        let schema = field_type_to_schema(&FieldType::Custom("MyType".to_string()));
        assert!(schema.is_empty());
    }

    #[test]
    fn fields_to_input_schema_basic() {
        // Mock: 既存 schemas/ping_pong.kdl と類似の構造を 直接 Field で組まずに、
        // 既存 KDL parser 経由で取得する
        let kdl = r#"
protocol "x" version="0.1.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "name" type="string" required=#true
            field "count" type="int"
            field "extra" type="json"
        }
    }
}
"#;
        let parsed = unison::parser::SchemaParser::new().parse(kdl).unwrap();
        let req = &parsed.protocol.as_ref().unwrap().channels[0].requests[0];
        let schema = fields_to_input_schema(&req.fields);

        assert_eq!(schema.get("type"), Some(&json!("object")));
        let props = schema.get("properties").and_then(Value::as_object).unwrap();
        assert_eq!(
            props
                .get("name")
                .and_then(Value::as_object)
                .unwrap()
                .get("type"),
            Some(&json!("string"))
        );
        assert_eq!(
            props
                .get("count")
                .and_then(Value::as_object)
                .unwrap()
                .get("type"),
            Some(&json!("integer"))
        );
        // json field は empty schema (= 任意)
        assert!(
            props
                .get("extra")
                .and_then(Value::as_object)
                .unwrap()
                .is_empty()
        );

        // required は name のみ
        let req_list = schema.get("required").and_then(Value::as_array).unwrap();
        assert_eq!(req_list, &vec![json!("name")]);

        // additionalProperties true
        assert_eq!(schema.get("additionalProperties"), Some(&json!(true)));
    }

    /// F11 (Purple Haze MEDIUM) acceptance: KDL `request "X" description="..."` が
    /// あれば、 synthesized tool の description として採用される (= formulaic を override)。
    /// LLM の tool-selection accuracy 改善のための入口。
    #[test]
    fn synthesize_tool_uses_kdl_description_when_present() {
        let kdl = r#"
protocol "x" version="0.1.0" {
    channel "memory" from="client" lifetime="persistent" {
        request "Search" description="Full-text search over user memories with semantic ranking" {
            field "query" type="string" required=#true
        }
    }
}
"#;
        let parsed = unison::parser::SchemaParser::new().parse(kdl).unwrap();
        let req = &parsed.protocol.as_ref().unwrap().channels[0].requests[0];
        let tool = synthesize_tool("memory", req);

        let desc = tool.description.as_ref().expect("description present");
        assert_eq!(
            desc.as_ref(),
            "Full-text search over user memories with semantic ranking",
            "KDL description should be used verbatim, not the formulaic default"
        );
    }

    /// description が無い場合は従来の formulaic default が使われる (= backward compat)。
    #[test]
    fn synthesize_tool_falls_back_to_formulaic_description() {
        let kdl = r#"
protocol "x" version="0.1.0" {
    channel "chat" from="client" lifetime="persistent" {
        request "Send" {
            field "msg" type="string" required=#true
        }
    }
}
"#;
        let parsed = unison::parser::SchemaParser::new().parse(kdl).unwrap();
        let req = &parsed.protocol.as_ref().unwrap().channels[0].requests[0];
        let tool = synthesize_tool("chat", req);

        let desc = tool.description.as_ref().expect("description present");
        assert!(desc.contains("Send"));
        assert!(desc.contains("chat"));
        assert!(
            desc.contains("runtime-synthesized"),
            "formulaic default should mention runtime synthesis"
        );
    }

    #[test]
    fn synthesize_tool_assembles_full_tool() {
        let kdl = r#"
protocol "x" version="0.1.0" {
    channel "chat" from="client" lifetime="persistent" {
        request "Send" {
            field "to" type="string" required=#true
            field "msg" type="string" required=#true
        }
    }
}
"#;
        let parsed = unison::parser::SchemaParser::new().parse(kdl).unwrap();
        let req = &parsed.protocol.as_ref().unwrap().channels[0].requests[0];
        let tool = synthesize_tool("chat", req);

        assert_eq!(tool.name.as_ref(), "unison_chat_Send");
        assert!(tool.description.as_ref().unwrap().contains("Send"));
        assert!(tool.description.as_ref().unwrap().contains("chat"));
        // input_schema は object with properties + required
        let schema_obj: &JsonObject = tool.input_schema.as_ref();
        assert_eq!(schema_obj.get("type"), Some(&json!("object")));
        let props = schema_obj
            .get("properties")
            .and_then(Value::as_object)
            .unwrap();
        assert!(props.contains_key("to"));
        assert!(props.contains_key("msg"));
    }

    /// F1 (Purple Haze) acceptance: KDL `type="array"` を持つ field が input_schema
    /// で `{"type": "array", "items": {}}` として出る (= 以前は parser が Custom("array")
    /// 扱いで empty schema、 dead code path だった)。
    #[test]
    fn synthesize_tool_with_array_field_produces_array_schema() {
        let kdl = r#"
protocol "x" version="0.1.0" {
    namespace "x"
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "tags" type="array" required=#true
            field "meta" type="map"
        }
    }
}
"#;
        let parsed = unison::parser::SchemaParser::new().parse(kdl).unwrap();
        let req = &parsed.protocol.as_ref().unwrap().channels[0].requests[0];
        let tool = synthesize_tool("c", req);
        let schema: &JsonObject = tool.input_schema.as_ref();
        let props = schema.get("properties").and_then(Value::as_object).unwrap();
        // array field
        let tags = props.get("tags").and_then(Value::as_object).unwrap();
        assert_eq!(tags.get("type"), Some(&json!("array")));
        assert!(
            tags.get("items").is_some(),
            "items property must exist for array"
        );
        // map field (= JSON Schema 慣用 = type:object + additionalProperties)
        let meta = props.get("meta").and_then(Value::as_object).unwrap();
        assert_eq!(meta.get("type"), Some(&json!("object")));
        assert!(meta.get("additionalProperties").is_some());
    }

    /// Anthropic Messages API `tools[].input_schema` 互換性: top-level に
    /// `type: "object"` + `properties` がある JSON Schema Draft 7+ object であること
    /// が要求される。 本 test で構造を検証 (= Hailing-δ leverage 1 acceptance)。
    #[test]
    fn input_schema_is_anthropic_compatible() {
        let kdl = r#"
protocol "x" version="0.1.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "name" type="string" required=#true
        }
    }
}
"#;
        let parsed = unison::parser::SchemaParser::new().parse(kdl).unwrap();
        let req = &parsed.protocol.as_ref().unwrap().channels[0].requests[0];
        let schema = fields_to_input_schema(&req.fields);

        // Anthropic input_schema 要件:
        // 1. top-level type は "object"
        assert_eq!(schema.get("type"), Some(&json!("object")));
        // 2. properties が object
        assert!(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some()
        );
        // 3. 全 properties の各 entry は object (= sub-schema)
        let props = schema.get("properties").and_then(Value::as_object).unwrap();
        for (_, v) in props {
            assert!(v.is_object(), "each property must be a JSON Schema object");
        }
        // 4. required は array of strings (= optional だが、 ある場合は string array)
        if let Some(req_list) = schema.get("required").and_then(Value::as_array) {
            for r in req_list {
                assert!(r.is_string(), "required entries must be strings");
            }
        }

        // JSON serialize できる (= MCP / Anthropic API どちらも JSON で transport)
        let json_str = serde_json::to_string(&schema).unwrap();
        let restored: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.get("type"), Some(&json!("object")));
    }
}
