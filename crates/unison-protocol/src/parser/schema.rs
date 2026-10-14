use club_kdl::KdlDeserialize;
use std::collections::HashMap;

/// Parsed schema representation
#[derive(Debug, Default, Clone, KdlDeserialize)]
#[kdl(document)]
pub struct ParsedSchema {
    #[kdl(child)]
    pub protocol: Option<Protocol>,

    #[kdl(children, name = "import")]
    pub imports: Vec<Import>,

    #[kdl(children, name = "message")]
    pub messages: Vec<Message>,

    #[kdl(children, name = "enum")]
    pub enums: Vec<Enum>,

    #[kdl(children, name = "typedef")]
    pub typedefs: Vec<TypeDef>,
}

/// Import definition
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "import")]
pub struct Import {
    #[kdl(argument)]
    pub path: String,
}

/// Protocol definition
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "protocol")]
pub struct Protocol {
    #[kdl(argument)]
    pub name: String,

    #[kdl(property)]
    pub version: String,

    #[kdl(child, unwrap_arg)]
    pub namespace: Option<String>,

    #[kdl(child, unwrap_arg)]
    pub description: Option<String>,

    #[kdl(children, name = "message")]
    pub messages: Vec<Message>,

    #[kdl(children, name = "enum")]
    pub enums: Vec<Enum>,

    #[kdl(children, name = "channel")]
    pub channels: Vec<Channel>,
}

/// Channel開始者
#[derive(Debug, Clone, PartialEq, KdlDeserialize)]
pub enum ChannelFrom {
    #[kdl(rename = "client")]
    Client,
    #[kdl(rename = "server")]
    Server,
    #[kdl(rename = "either")]
    Either,
}

/// Channelの寿命
#[derive(Debug, Clone, PartialEq, KdlDeserialize)]
pub enum ChannelLifetime {
    #[kdl(rename = "transient")]
    Transient,
    #[kdl(rename = "persistent")]
    Persistent,
}

/// Channel の wire backend (v0.10.0 で追加)
///
/// `Stream` は QUIC bidi stream に対応 (= ordered + reliable)、 `Datagram` は QUIC
/// datagram に対応 (= unordered + unreliable + ≤MTU)。 `Datagram` の場合は
/// [`Channel::channel_id`] が必須 (= varint prefix で demux)。 default は `Stream`。
///
/// 詳細は `design/datagram-channel.md` および `spec/02-unified-channel/SPEC.md` §8.5 参照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, KdlDeserialize)]
pub enum ChannelBackend {
    /// QUIC bidi stream (= ordered + reliable、 v0.9.0 までの唯一の backend)
    #[default]
    #[kdl(rename = "stream")]
    Stream,
    /// QUIC datagram (= unordered + unreliable、 ≤MTU、 v0.10.0 で追加)
    #[kdl(rename = "datagram")]
    Datagram,
}

/// Channel内のメッセージ定義（名前付き）
#[derive(Debug, Clone, KdlDeserialize)]
pub struct ChannelMessage {
    /// メッセージ名
    #[kdl(argument)]
    pub name: String,

    /// フィールド定義
    #[kdl(children, name = "field")]
    pub fields: Vec<Field>,
}

/// チャネル内 Request/Response 定義
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "request")]
pub struct ChannelRequest {
    /// リクエスト名
    #[kdl(argument)]
    pub name: String,

    /// リクエストの人間可読な説明 (= MCP tool description 等に流す)。
    /// optional、 KDL syntax は `request "Name" description="..." { ... }`。
    #[kdl(property)]
    pub description: Option<String>,

    /// 環境を変更しない読み取り専用リクエストであることの宣言 (= safety hint)。
    ///
    /// optional。 server 作者が自チャネルの安全性を宣言し、 AI agent 等の
    /// consumer (= unison-mcp の `ToolAnnotations` 合成など) が尊重する。
    /// 未宣言 (= `None`) は「不明」であり consumer 側の default に委ねる。
    /// `destructive=#true` との同時宣言は矛盾として validation error。
    #[kdl(property)]
    pub readonly: Option<bool>,

    /// 破壊的更新 (= 復元不能な削除・上書き) があり得ることの宣言 (= safety hint)。
    ///
    /// optional。 semantics は [`Self::readonly`] と同じ hint 系。
    #[kdl(property)]
    pub destructive: Option<bool>,

    /// 同一引数での再実行が追加の効果を持たないことの宣言 (= safety hint)。
    ///
    /// optional。 semantics は [`Self::readonly`] と同じ hint 系。
    #[kdl(property)]
    pub idempotent: Option<bool>,

    /// リクエストフィールド
    #[kdl(children, name = "field")]
    pub fields: Vec<Field>,

    /// レスポンス型（returns ブロック）
    #[kdl(child)]
    pub returns: Option<ChannelMessage>,
}

/// チャネル内 Event 定義
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "event")]
pub struct ChannelEvent {
    /// イベント名
    #[kdl(argument)]
    pub name: String,

    /// イベントフィールド
    #[kdl(children, name = "field")]
    pub fields: Vec<Field>,
}

/// Channel定義（Unified Channel プリミティブ）
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "channel")]
pub struct Channel {
    /// チャネル名
    #[kdl(argument)]
    pub name: String,

    /// 誰がStreamを開くか
    #[kdl(property)]
    pub from: ChannelFrom,

    /// Streamの寿命
    #[kdl(property)]
    pub lifetime: ChannelLifetime,

    /// Wire backend (= `"stream"` (default) / `"datagram"`、 v0.10.0 で追加)
    ///
    /// 省略時は [`ChannelBackend::Stream`] (= v0.9.0 schema 互換動作)。
    /// [`ChannelBackend::Datagram`] の場合は [`Self::channel_id`] が必須。
    /// 取得は [`Self::backend`] で実行 (= Option を unwrap)。
    #[kdl(property)]
    pub backend: Option<ChannelBackend>,

    /// Datagram channel の demux 識別子 (v0.10.0 で追加)
    ///
    /// `backend="datagram"` のとき必須、 1.. の正整数。 wire 上は varint encoded
    /// prefix として datagram payload 先頭に乗る。 author 明示割り当て (= proto3
    /// field number 哲学)、 schema reorder で値を変えると wire format breaking。
    #[kdl(property)]
    pub channel_id: Option<u64>,

    /// Request/Response 定義（新構文）
    #[kdl(children, name = "request")]
    pub requests: Vec<ChannelRequest>,

    /// Event 定義
    #[kdl(children, name = "event")]
    pub events: Vec<ChannelEvent>,
}

impl Channel {
    /// この channel の backend を取得 (= default は [`ChannelBackend::Stream`])
    pub fn backend(&self) -> ChannelBackend {
        self.backend.unwrap_or_default()
    }

    /// Channel の semantic validation を行う。
    ///
    /// 検証項目:
    /// - `backend="datagram"` の場合は `channel_id` が必須、 0 は予約 (= sentinel)
    /// - `backend="stream"` (= default) の場合は `channel_id` を指定しても無視 (= warning は出さない)
    /// - `backend="datagram"` の channel は `request` ブロックを持てない (= datagram は応答不可)
    /// - request の safety hint は `readonly=#true` と `destructive=#true` を同時宣言できない (= 矛盾)
    pub fn validate(&self) -> Result<(), String> {
        for request in &self.requests {
            if request.readonly == Some(true) && request.destructive == Some(true) {
                return Err(format!(
                    "request \"{}\" in channel \"{}\" declares both readonly=#true and \
                     destructive=#true; a readonly request cannot be destructive — pick one",
                    request.name, self.name
                ));
            }
        }
        match self.backend() {
            ChannelBackend::Datagram => {
                let id = self.channel_id.ok_or_else(|| {
                    format!(
                        "channel \"{}\" has backend=\"datagram\" but no channel_id; \
                             explicit channel_id=N (1..) is required",
                        self.name
                    )
                })?;
                if id == 0 {
                    return Err(format!(
                        "channel \"{}\" has channel_id=0 which is reserved; use 1..",
                        self.name
                    ));
                }
                if !self.requests.is_empty() {
                    return Err(format!(
                        "channel \"{}\" has backend=\"datagram\" with request blocks; \
                         datagram channels support event only (= no Request/Response)",
                        self.name
                    ));
                }
                Ok(())
            }
            ChannelBackend::Stream => Ok(()),
        }
    }
}

/// Message/struct definition
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "message")]
pub struct Message {
    #[kdl(argument)]
    pub name: String,

    #[kdl(child, unwrap_arg)]
    pub description: Option<String>,

    #[kdl(children, name = "field")]
    pub fields: Vec<Field>,
}

/// Field definition (KDL representation)
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "field")]
pub struct Field {
    #[kdl(argument)]
    pub name: String,

    #[kdl(property, rename = "type")]
    pub field_type_str: String,

    #[kdl(property, default)]
    pub required: bool,

    #[kdl(property, rename = "default")]
    pub default_str: Option<String>,

    /// `type="array"` の要素型 (= `field "xs" type="array" items="Atlas"`)。
    ///
    /// 省略時は任意 JSON 要素。 型名の解釈は [`Field::field_type`] が
    /// scalar 名 / 宣言済み名 (= `Custom`) として行う。
    #[kdl(property)]
    pub items: Option<String>,

    /// `type="map"` のキー型 (= `field "m" type="map" keys="string"`)。
    ///
    /// 省略時は `string` (= JSON object のキーは常に文字列)。
    #[kdl(property)]
    pub keys: Option<String>,

    /// `type="map"` の値型 (= `field "m" type="map" values="Label"`)。
    ///
    /// 省略時は任意 JSON 値。
    #[kdl(property)]
    pub values: Option<String>,

    #[kdl(property)]
    pub min: Option<i64>,

    #[kdl(property)]
    pub max: Option<i64>,

    #[kdl(property)]
    pub min_length: Option<usize>,

    #[kdl(property)]
    pub max_length: Option<usize>,

    #[kdl(property)]
    pub pattern: Option<String>,

    #[kdl(property)]
    pub description: Option<String>,
}

impl Field {
    /// フィールド型を取得
    pub fn field_type(&self) -> FieldType {
        self.parse_field_type(&self.field_type_str)
    }

    /// デフォルト値を取得
    pub fn default(&self) -> Option<DefaultValue> {
        self.default_str
            .as_ref()
            .and_then(|s| self.parse_default(s))
    }

    /// 制約を取得
    pub fn constraints(&self) -> Constraints {
        Constraints {
            min: self.min,
            max: self.max,
            min_length: self.min_length,
            max_length: self.max_length,
            pattern: self.pattern.clone(),
        }
    }

    fn parse_field_type(&self, type_str: &str) -> FieldType {
        match type_str {
            "string" => FieldType::String,
            "int" => FieldType::Int,
            "float" => FieldType::Float,
            "bool" => FieldType::Bool,
            "json" => FieldType::Json,
            "object" => FieldType::Object,
            // `type="array"` = JSON array。 要素型は `items="T"` で書ける
            // (= 省略時は任意 JSON)。 `array<T>` という string 構文は採らない
            // (KDL の property で書く方が parser に小さく収まる)。
            "array" => FieldType::Array(Box::new(self.element_type(self.items.as_deref()))),
            // `type="map"` = JSON object。 キーは常に文字列なので `keys` 省略時は
            // `string`、 値型は `values="T"` で書ける (= 省略時は任意 JSON)。
            "map" => FieldType::Map(
                Box::new(
                    self.keys
                        .as_deref()
                        .map_or(FieldType::String, |k| self.parse_field_type(k)),
                ),
                Box::new(self.element_type(self.values.as_deref())),
            ),
            _ => FieldType::Custom(type_str.to_string()),
        }
    }

    /// `items` / `values` の型名を解釈する (= 未指定なら任意 JSON)。
    fn element_type(&self, name: Option<&str>) -> FieldType {
        name.map_or(FieldType::Json, |n| self.parse_field_type(n))
    }

    fn parse_default(&self, s: &str) -> Option<DefaultValue> {
        // 簡易的なパース実装
        if s == "null" {
            Some(DefaultValue::Null)
        } else if s == "true" {
            Some(DefaultValue::Bool(true))
        } else if s == "false" {
            Some(DefaultValue::Bool(false))
        } else if let Ok(i) = s.parse::<i64>() {
            Some(DefaultValue::Int(i))
        } else if let Ok(f) = s.parse::<f64>() {
            Some(DefaultValue::Float(f))
        } else {
            Some(DefaultValue::String(s.to_string()))
        }
    }
}

/// Field type
#[derive(Debug, Clone)]
pub enum FieldType {
    String,
    Int,
    Float,
    Bool,
    Json,
    Array(Box<FieldType>),
    Map(Box<FieldType>, Box<FieldType>),
    Enum(Vec<String>),
    Object,
    Custom(String),
}

/// Enum definition
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "enum")]
pub struct Enum {
    #[kdl(argument)]
    pub name: String,

    #[kdl(child, unwrap_args)]
    pub values: Vec<String>,
}

/// Type definition
#[derive(Debug, Clone, KdlDeserialize)]
#[kdl(name = "typedef")]
pub struct TypeDef {
    #[kdl(argument)]
    pub name: String,

    #[kdl(child, unwrap_arg)]
    pub base_type: String,

    #[kdl(child, unwrap_arg)]
    pub rust_type: Option<String>,

    #[kdl(child, unwrap_arg)]
    pub typescript_type: Option<String>,

    #[kdl(child, unwrap_arg)]
    pub format: Option<String>,

    #[kdl(child, unwrap_arg)]
    pub pattern: Option<String>,
}

/// Default value for fields
#[derive(Debug, Clone)]
pub enum DefaultValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<DefaultValue>),
    Object(HashMap<String, DefaultValue>),
    Null,
}

/// Field constraints
#[derive(Debug, Clone, Default)]
pub struct Constraints {
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub pattern: Option<String>,
}
