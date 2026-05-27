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

/// channel name と method から MCP tool name を組み立てる。
///
/// 規約: `unison_<channel_safe>_<method>` where channel_safe は `.` と `-` を `_` に置換。
///
/// # Example
///
/// ```ignore
/// assert_eq!(synth_tool_name("chat", "Send"), "unison_chat_Send");
/// assert_eq!(synth_tool_name("unison.discovery", "GetProtocol"), "unison_unison_discovery_GetProtocol");
/// ```
pub fn synth_tool_name(channel: &str, method: &str) -> String {
    let safe = normalize_channel_name(channel);
    format!("{SYNTH_TOOL_PREFIX}{safe}_{method}")
}

/// channel name を tool name に使える形に正規化する (= `.` `-` → `_`)
fn normalize_channel_name(channel: &str) -> String {
    channel.replace(['.', '-'], "_")
}

/// tool name から (channel, method) を逆引きする。
///
/// 「合致する channel が registry に存在する」 ことを caller が確認する想定。 本関数は
/// 文字列分解のみ行い、 妥当性検査は行わない。
///
/// 候補が複数ある場合 (= channel name に `_` が含まれて method 境界が曖昧)、
/// 「channel が registry にある」 で絞り込む caller 側の責務。
///
/// # 戻り値
/// 候補 (channel, method) の Vec (= prefix が `unison_` で始まる場合のみ)。 prefix
/// なしや split できない名前は空 Vec。
pub fn parse_tool_name(name: &str) -> Vec<(String, String)> {
    let Some(rest) = name.strip_prefix(SYNTH_TOOL_PREFIX) else {
        return Vec::new();
    };
    // 最後の `_` を method 境界とする 1 候補に加え、 すべての `_` 位置で split した
    // 候補も含める (= 例えば "chat_Send" → ("chat", "Send"), "unison_discovery_GetProtocol"
    // → ("unison_discovery", "GetProtocol"), ("unison", "discovery_GetProtocol") 等)
    let mut out = Vec::new();
    let positions: Vec<usize> = rest
        .char_indices()
        .filter(|(_, c)| *c == '_')
        .map(|(i, _)| i)
        .collect();
    for &i in &positions {
        let channel_normalized = &rest[..i];
        let method = &rest[i + 1..];
        if !channel_normalized.is_empty() && !method.is_empty() {
            out.push((channel_normalized.to_string(), method.to_string()));
        }
    }
    out
}

/// registry を見て、 候補 (channel, method) から実際に存在する組を選ぶ。
///
/// `parse_tool_name` の候補 Vec と、 registry の channel name 集合 (= 正規化前) を
/// 受け取り、 「channel name を正規化したものが候補 channel と一致するもの」 で最初に
/// マッチする (channel, method) を返す。
pub fn resolve_tool_name<'a, I>(name: &str, channels: I) -> Option<(String, String)>
where
    I: IntoIterator<Item = &'a str>,
{
    let candidates = parse_tool_name(name);
    if candidates.is_empty() {
        return None;
    }
    let channel_map: Vec<(&str, String)> =
        channels.into_iter().map(|c| (c, normalize_channel_name(c))).collect();
    for (candidate_chan_norm, method) in candidates {
        for (raw, normalized) in &channel_map {
            if *normalized == candidate_chan_norm {
                return Some(((*raw).to_string(), method));
            }
        }
    }
    None
}

/// `ChannelRequest` を MCP `Tool` に変換する。
///
/// - name: `synth_tool_name(channel, request.name)`
/// - description: 「`<channel>.<method>` channel request」 (= KDL に description field
///   があれば優先する future hint)
/// - input_schema: `request.fields` を JSON Schema object に変換
pub fn synthesize_tool(channel_name: &str, request: &ChannelRequest) -> Tool {
    let name = synth_tool_name(channel_name, &request.name);
    let description = format!(
        "Invoke the `{}` request on the `{}` Unison channel (= runtime-synthesized from server's KDL schema).",
        request.name, channel_name
    );
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
            obj.insert(
                "additionalProperties".to_string(),
                Value::Object(v_schema),
            );
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
        assert_eq!(synth_tool_name("ping-pong", "Ping"), "unison_ping_pong_Ping");
    }

    #[test]
    fn parse_tool_name_returns_candidates() {
        let candidates = parse_tool_name("unison_chat_Send");
        assert!(candidates.contains(&("chat".to_string(), "Send".to_string())));
    }

    #[test]
    fn parse_tool_name_handles_multi_underscore() {
        let candidates = parse_tool_name("unison_unison_discovery_GetProtocol");
        // 候補に ("unison_discovery", "GetProtocol") が含まれる
        assert!(
            candidates
                .iter()
                .any(|(c, m)| c == "unison_discovery" && m == "GetProtocol")
        );
    }

    #[test]
    fn parse_tool_name_no_prefix_returns_empty() {
        // prefix なし
        assert!(parse_tool_name("chat_Send").is_empty());
        // prefix `unison_` 剥がし後の "ping" には `_` が無いので候補なし
        // (= static escape hatch tool 名は parse_tool_name の対象外、 caller 側で先に
        //    static 名を check してから synthesized lookup に進む dispatch 構造)
        assert!(parse_tool_name("unison_ping").is_empty());
        // prefix のみで body が空 → 空
        assert!(parse_tool_name("unison_").is_empty());
    }

    #[test]
    fn resolve_tool_name_against_registered_channels() {
        let channels = ["chat", "unison.discovery", "metrics"];
        let resolved = resolve_tool_name("unison_unison_discovery_GetProtocol", channels)
            .expect("should resolve");
        assert_eq!(resolved.0, "unison.discovery");
        assert_eq!(resolved.1, "GetProtocol");

        let resolved = resolve_tool_name("unison_chat_Send", channels).expect("should resolve");
        assert_eq!(resolved.0, "chat");
        assert_eq!(resolved.1, "Send");
    }

    #[test]
    fn resolve_tool_name_unknown_channel_returns_none() {
        let channels = ["chat"];
        assert!(resolve_tool_name("unison_ghost_X", channels).is_none());
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
        let ap = schema.get("additionalProperties").and_then(Value::as_object).unwrap();
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
            props.get("name").and_then(Value::as_object).unwrap().get("type"),
            Some(&json!("string"))
        );
        assert_eq!(
            props.get("count").and_then(Value::as_object).unwrap().get("type"),
            Some(&json!("integer"))
        );
        // json field は empty schema (= 任意)
        assert!(props.get("extra").and_then(Value::as_object).unwrap().is_empty());

        // required は name のみ
        let req_list = schema.get("required").and_then(Value::as_array).unwrap();
        assert_eq!(req_list, &vec![json!("name")]);

        // additionalProperties true
        assert_eq!(schema.get("additionalProperties"), Some(&json!(true)));
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
        let props = schema_obj.get("properties").and_then(Value::as_object).unwrap();
        assert!(props.contains_key("to"));
        assert!(props.contains_key("msg"));
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
        assert!(schema.get("properties").and_then(Value::as_object).is_some());
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
