# KDL → JSON Schema 対応表

> **Status**: v0.1.0 (Unison Hailing α P3b 一部)
> **Implementation**: `crates/unison-mcp/src/mapping.rs::field_type_to_schema`
> **Consumers**: MCP `Tool.input_schema` / Anthropic Messages API `tools[].input_schema` / OpenAI `response_format` / Vercel AI SDK `generateObject` / Instructor / outlines

## 1. なぜこの対応表が要るか

KDL は Unison の SSOT として channel/request/event の型を語る。 これを **runtime で JSON Schema に変換** することで:

- MCP tool の `input_schema` (= AI agent が引数を作る際の制約) として流せる
- Anthropic Messages API の `tools[].input_schema` でも同じ JSON Schema が valid (= structured tool use)
- OpenAI Structured Outputs / Vercel AI SDK / Instructor などの **「JSON Schema を共通通貨にする ecosystem」** にも乗れる

→ 「converter は 1 つ、 consumer は複数」 という Hailing-δ leverage 1 の core。

## 2. Field 型対応表 (= 単一型)

| KDL `FieldType` | JSON Schema 出力 | 説明 |
|---|---|---|
| `String` | `{"type": "string"}` | UTF-8 文字列 |
| `Int` | `{"type": "integer"}` | 整数。 JSON Schema の `integer` (= JSON number で整数判定される値) |
| `Float` | `{"type": "number"}` | 浮動小数。 JSON Schema の `number` (= 整数を含む数値全般) |
| `Bool` | `{"type": "boolean"}` | 真偽値 |
| `Json` | `{}` (= empty) | **任意の JSON 値** (= type 制約なし)。 schema-less 領域 |
| `Object` | `{"type": "object"}` | JSON object (= properties 不問) |
| `Array(inner)` | `{"type": "array", "items": <inner>}` | 要素型を再帰的に展開 |
| `Map(K, V)` | `{"type": "object", "additionalProperties": <V>}` | JSON Schema の慣用 (= dynamic key の object)。 K は基本的に string で固定 |
| `Custom(name)` | `{}` (= empty) | **passthrough** in v0.1.0。 typedef 解決は将来 |
| `Enum(name)` | `{}` (= empty) | **passthrough** in v0.1.0。 enum 値展開は将来 |

### Lossy 部分 (= v0.1.0 では fully captured されない)

- `Custom` / `Enum` — typedef 解決が無いため、 LLM 側は何でも生成できてしまう (= server-side validation が必須)
- `Float` の sub-type 区別 (= f32 vs f64) — JSON Schema は number 1 種類
- KDL field の制約 (= `min`, `max`, `min_length`, `max_length`, `pattern`) — v0.1.0 では schema に出力しない (= constraint 検証は SchemaRegistry 側でも未実装)
- `Int` の sub-type 区別 (= i32 vs i64) — JSON Schema は integer 1 種類、 範囲は constraint で表現可能

## 3. Request schema 構築 (= 複数 field の組み合わせ)

`fields_to_input_schema(fields: &[Field]) -> JsonObject` の出力:

```json
{
  "type": "object",
  "properties": {
    "<field_name>": { ... 上記対応表で展開 ... },
    ...
  },
  "required": ["<required_field_1>", ...],
  "additionalProperties": true
}
```

### Field-level 修飾

| KDL | JSON Schema 出力 |
|---|---|
| `field "name" type="string" required=#true` | `properties.name = {"type": "string"}` + `required` 配列に `"name"` 追加 |
| `field "name" type="string"` (省略) | `properties.name = {"type": "string"}` + `required` 配列に **含めない** |
| `field "name" type="string" description="..."` | `properties.name = {"type": "string", "description": "..."}` |
| `field "name" type="..." default=...` | v0.1.0 では schema に出力しない (= server 側 default として扱う) |
| `field "name" type="..." min=N max=N` | v0.1.0 では schema に出力しない (= future expansion) |

### `additionalProperties: true` の意図

**forward-compat**: schema に無い field を payload に含めても reject しない。 これにより:

- KDL を server 側で field 追加 → client が古い schema を持っていても破壊しない
- AI agent が experimental field を試行錯誤しても通る (= LLM の柔軟性)

逆効果 (= strict mode が欲しいケース) は別 Epic / opt-in で対応。

## 4. Example

### KDL 入力

```kdl
channel "chat" from="client" lifetime="persistent" {
    request "Send" {
        field "to" type="string" required=#true description="Recipient address"
        field "msg" type="string" required=#true
        field "reply_to" type="string"
        field "tags" type="array" {
            // 注: 現状 parser は array の要素型を 「`Array(Box<FieldType>)`」 として
            //     扱うが、 KDL syntax で要素型を指定する記法は未確立 (= 課題)
        }
        returns "Ack" {
            field "ok" type="bool" required=#true
        }
    }
}
```

### MCP `Tool.input_schema` 出力

```json
{
  "type": "object",
  "properties": {
    "to": {
      "type": "string",
      "description": "Recipient address"
    },
    "msg": {
      "type": "string"
    },
    "reply_to": {
      "type": "string"
    },
    "tags": {
      "type": "array",
      "items": {}
    }
  },
  "required": ["to", "msg"],
  "additionalProperties": true
}
```

### Anthropic Messages API での使用 (= 同 JSON Schema をそのまま)

```python
client.messages.create(
    model="claude-opus-4-7",
    tools=[{
        "name": "unison_chat_Send",
        "description": "Invoke the `Send` request on the `chat` Unison channel",
        "input_schema": <上の JSON Schema>,  # ← そのまま流せる
    }],
    ...
)
```

## 5. 将来拡張

| 項目 | 想定 phase / Epic |
|---|---|
| **Typed element syntax** (= `array<string>`、 `map<string, int>` 等) — 現状は `type="array"` / `type="map"` のみ recognize、 要素型は untyped (= `Json`) | 別 Epic、 KDL syntax convention の確立と連動 |
| `Custom` / `Enum` の typedef 解決 (= 具体型を schema に展開) | 後続 phase、 KDL parser 側の typedef AST 整備と連動 |
| `Float` の f32/f64 sub-type 区別 | JSON Schema の `format` 拡張で表現可 (= `float`/`double`) |
| 制約 (`min`/`max`/`pattern`) の schema 出力 | constraint Epic で SchemaRegistry validation と同時に追加 |
| `additionalProperties: false` の strict mode | strict-validation opt-in (= 別 channel attribute or call site flag) |
| 返り値 (`returns`) の schema → MCP `Tool.output_schema` | P3c 以降の polish phase |
| KDL `description` の channel/request level 集約 | tool description の自動充実 |

## 6. テスト位置

| test | 場所 | 検証内容 |
|---|---|---|
| 基本型 → schema 変換 | `mapping.rs` 内 `field_type_*_to_schema` 各 unit test | 個別 FieldType の正しい変換 |
| Request → input schema 構築 | `mapping.rs` 内 `fields_to_input_schema_basic` | properties + required + additionalProperties |
| Anthropic compat | `mapping.rs` 内 `input_schema_is_anthropic_compatible` | top-level type=object + JSON serializable |
| E2E synthesis | `crates/unison-mcp/tests/test_integ_synthesis.rs` | discovery server → UnisonMcp → list_tools / invoke_tool |

## 7. 参照

- 実装: `crates/unison-mcp/src/mapping.rs`
- KDL parser AST: `crates/unison-protocol/src/parser/schema.rs::FieldType`
- 設計 spark: Unison Hailing-δ (LLM-native payload generation) = `mem_1CbSyUr2Pg4dY6Yq3Gkztt`
- 関連 spec: `spec/04-discovery/SPEC.md` §8 (= client 側 TypeRegistry 構築)
