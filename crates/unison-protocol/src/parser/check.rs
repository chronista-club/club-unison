//! JSON が約束 (KDL) の型を満たすかの判定 (= `unison schema-check` の核)。
//!
//! 規則は「client が応答として decode するときに失敗する条件」を誤りにする:
//!
//! - required の field が欠けている、 または null
//! - 型が違う (`int` に小数、 `string` に数 など)
//! - `items=` / `values=` の要素の型が違う (要素の null も、 要素型が `json` でなければ誤り)
//! - 入れ子の message が約束を満たさない
//! - enum に無い値
//! - 約束に宣言の無い型名 (= 約束側の誤り。 黙って通さない)
//!
//! 知らない key、 任意 field の欠落 / null は通す。 `int` は `3.0` のように小数部が 0 の数を
//! 通し、 `float` は整数を通す (= 数値としての decode が成り立つため)。
//!
//! 判定の規則を 1 か所に置くための型で、 約束を書く側 (例: Creo の REST) は規則を持たない。

use std::collections::HashMap;

use serde_json::Value;

use super::{Field, FieldType, ParsedSchema};

/// typedef をたどる深さの上限 (= `typedef "A" { base_type "B" }` と B → A の循環で止まる)。
const MAX_TYPEDEF_DEPTH: usize = 32;

/// 約束の型を名前で引く索引 (message / enum / typedef)。
pub struct TypeIndex {
    messages: HashMap<String, Vec<Field>>,
    enums: HashMap<String, Vec<String>>,
    typedefs: HashMap<String, String>,
}

/// 約束を満たさない箇所。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// JSONPath 風の場所 (`$.memories[1].content` / `$.labels["c d"]`)
    pub path: String,
    /// 何が約束と違うか
    pub reason: String,
}

impl Violation {
    fn new(path: &str, reason: impl Into<String>) -> Self {
        Self {
            path: path.to_string(),
            reason: reason.into(),
        }
    }
}

impl TypeIndex {
    /// document 直下と protocol 直下の message / enum、 typedef、 channel の中の
    /// `returns` / `event` を索引にする。
    pub fn new(schema: &ParsedSchema) -> Self {
        let protocol = schema.protocol.as_ref();
        let mut messages: HashMap<String, Vec<Field>> = schema
            .messages
            .iter()
            .chain(protocol.into_iter().flat_map(|p| &p.messages))
            .map(|m| (m.name.clone(), m.fields.clone()))
            .collect();
        // channel の中で client が decode する形 (= `returns` / `event`) も名前で引く。
        // 同名の top-level message があればそちらを優先し、 inline 同士の重複は先勝ち。
        for ch in protocol.into_iter().flat_map(|p| &p.channels) {
            let returns = ch
                .requests
                .iter()
                .filter_map(|r| r.returns.as_ref())
                .map(|r| (&r.name, &r.fields));
            let events = ch.events.iter().map(|e| (&e.name, &e.fields));
            for (name, fields) in returns.chain(events) {
                messages
                    .entry(name.clone())
                    .or_insert_with(|| fields.clone());
            }
        }
        let enums = schema
            .enums
            .iter()
            .chain(protocol.into_iter().flat_map(|p| &p.enums))
            .map(|e| (e.name.clone(), e.values.clone()))
            .collect();
        let typedefs = schema
            .typedefs
            .iter()
            .map(|t| (t.name.clone(), t.base_type.clone()))
            .collect();
        Self {
            messages,
            enums,
            typedefs,
        }
    }

    /// `value` を message `message` として読めるかを判定し、 約束を満たさない箇所を返す。
    ///
    /// 空なら満たしている。 message が約束に無いときは `$` に 1 件返す。
    pub fn check(&self, message: &str, value: &Value) -> Vec<Violation> {
        let mut out = Vec::new();
        match self.messages.get(message) {
            Some(fields) => self.check_object(fields, value, "$", &mut out),
            None => out.push(Violation::new(
                "$",
                format!("message \"{message}\" is not declared in the contract"),
            )),
        }
        out
    }

    fn check_object(&self, fields: &[Field], value: &Value, path: &str, out: &mut Vec<Violation>) {
        let Some(obj) = value.as_object() else {
            out.push(Violation::new(
                path,
                format!("expected an object, got {}", kind(value)),
            ));
            return;
        };
        for field in fields {
            let at = child_key(path, &field.name);
            match obj.get(&field.name) {
                None if field.required => {
                    out.push(Violation::new(&at, "required field is missing"))
                }
                Some(Value::Null) if field.required => {
                    out.push(Violation::new(&at, "required field is null"))
                }
                None | Some(Value::Null) => {}
                Some(v) => self.check_type(&field.field_type(), v, &at, 0, out),
            }
        }
    }

    fn check_type(
        &self,
        ty: &FieldType,
        value: &Value,
        path: &str,
        depth: usize,
        out: &mut Vec<Violation>,
    ) {
        let ok = match ty {
            FieldType::String => value.is_string(),
            FieldType::Int => is_integral(value),
            FieldType::Float => value.is_number(),
            FieldType::Bool => value.is_boolean(),
            FieldType::Json => true,
            FieldType::Object => value.is_object(),
            FieldType::Enum(values) => value
                .as_str()
                .is_some_and(|s| values.iter().any(|v| v == s)),
            FieldType::Array(item) => {
                match value.as_array() {
                    Some(items) => {
                        for (i, v) in items.iter().enumerate() {
                            self.check_element(item, v, &format!("{path}[{i}]"), depth, out);
                        }
                    }
                    None => out.push(mismatch(ty, value, path)),
                }
                return;
            }
            FieldType::Map(_, values) => {
                match value.as_object() {
                    Some(entries) => {
                        for (k, v) in entries {
                            self.check_element(values, v, &child_key(path, k), depth, out);
                        }
                    }
                    None => out.push(mismatch(ty, value, path)),
                }
                return;
            }
            FieldType::Custom(name) => {
                self.check_named(name, value, path, depth, out);
                return;
            }
        };
        if !ok {
            out.push(mismatch(ty, value, path));
        }
    }

    /// 配列 / map の要素。 null は要素型が `json` のときだけ通す。
    fn check_element(
        &self,
        ty: &FieldType,
        value: &Value,
        path: &str,
        depth: usize,
        out: &mut Vec<Violation>,
    ) {
        if value.is_null() && !matches!(ty, FieldType::Json) {
            out.push(Violation::new(path, "element is null"));
            return;
        }
        self.check_type(ty, value, path, depth, out);
    }

    /// 宣言名 (message / enum / typedef) の型で判定する。
    fn check_named(
        &self,
        name: &str,
        value: &Value,
        path: &str,
        depth: usize,
        out: &mut Vec<Violation>,
    ) {
        if let Some(fields) = self.messages.get(name) {
            self.check_object(fields, value, path, out);
        } else if let Some(values) = self.enums.get(name) {
            match value.as_str() {
                Some(s) if values.iter().any(|v| v == s) => {}
                Some(s) => out.push(Violation::new(
                    path,
                    format!("\"{s}\" is not a value of enum \"{name}\""),
                )),
                None => out.push(Violation::new(
                    path,
                    format!("expected a string (enum \"{name}\"), got {}", kind(value)),
                )),
            }
        } else if let Some(base) = self.typedefs.get(name) {
            if depth >= MAX_TYPEDEF_DEPTH {
                out.push(Violation::new(
                    path,
                    format!("typedef \"{name}\" does not resolve to a type (cycle?)"),
                ));
                return;
            }
            self.check_type(&named_type(base), value, path, depth + 1, out);
        } else {
            out.push(Violation::new(
                path,
                format!("type \"{name}\" is not declared in the contract"),
            ));
        }
    }
}

/// typedef の `base_type` の型名を解釈する (= 要素型の指定は持たない)。
fn named_type(name: &str) -> FieldType {
    match name {
        "string" => FieldType::String,
        "int" => FieldType::Int,
        "float" => FieldType::Float,
        "bool" => FieldType::Bool,
        "json" => FieldType::Json,
        "object" => FieldType::Object,
        "array" => FieldType::Array(Box::new(FieldType::Json)),
        "map" => FieldType::Map(Box::new(FieldType::String), Box::new(FieldType::Json)),
        other => FieldType::Custom(other.to_string()),
    }
}

/// 整数として decode できる数 (= 小数部が 0 の浮動小数も含む)。
fn is_integral(value: &Value) -> bool {
    value.is_i64()
        || value.is_u64()
        || value
            .as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

fn mismatch(ty: &FieldType, value: &Value, path: &str) -> Violation {
    // 数と bool は値も添える (= `expected int, got number 3.5`)
    let got = match value {
        Value::Number(_) | Value::Bool(_) => format!("{} {value}", kind(value)),
        _ => kind(value).to_string(),
    };
    Violation::new(path, format!("expected {}, got {got}", type_label(ty)))
}

fn type_label(ty: &FieldType) -> String {
    match ty {
        FieldType::String => "string".into(),
        FieldType::Int => "int".into(),
        FieldType::Float => "float".into(),
        FieldType::Bool => "bool".into(),
        FieldType::Json => "json".into(),
        FieldType::Object => "object".into(),
        FieldType::Array(e) => format!("array<{}>", type_label(e)),
        FieldType::Map(k, v) => format!("map<{},{}>", type_label(k), type_label(v)),
        FieldType::Enum(vs) => format!("one of {}", vs.join(" / ")),
        FieldType::Custom(n) => n.clone(),
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// object の key を path に足す (= 識別子なら `.key`、 それ以外は `["key"]`)。
fn child_key(path: &str, key: &str) -> String {
    let is_ident = key
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if is_ident {
        format!("{path}.{key}")
    } else {
        // serde_json の文字列表現で quote / escape する
        format!("{path}[{}]", Value::String(key.to_string()))
    }
}
