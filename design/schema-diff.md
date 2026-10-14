# schema-diff — 配った client の約束を壊していないかを確かめる

**Status**: Draft（実装済み、`unison schema-diff`）
**Related**: [spec/02](../spec/02-unified-channel/SPEC.md) §4（field の型と修飾子）、creo-memories spec 34（api-contract-for-shipped-clients、利用側）
**対象**: `crates/unison-cli/src/schema_diff.rs`、`crates/unison-protocol/src/parser/schema.rs`（`items` / `keys` / `values`）

---

## Abstract

配布済みの client（例: Creo iOS 1.0.1 build 11）は、利用者が更新するまで同じ前提で server を叩き続ける。server が変わり続けても古い client の前提を壊していないことを、CI で毎回確かめたい。

`unison schema-diff <old.kdl> <new.kdl>` は、旧版（= 配った client が前提にしている約束）と新版（= 今の server の約束）を比べ、壊れる変更があれば exit 1 を返す。判定の要点は **向き（要求か応答か）で規則が逆になる** こと。

---

## Architecture

### 向きの決め方

向きは **旧版** の参照関係から決める。種は 2 系統。

| 種 | 向き |
|---|---|
| channel の `request` の field | 要求（client → server） |
| `returns` の field | 応答（server → client） |
| `event` の field | channel の `from` で決まる（`server` → 応答、`client` → 要求、`either` → 両方） |
| 方言に無い node（とその子孫）の `request="X"` / `response="Y"` | X を要求、Y を応答 |

2 つ目の系統は、**Unison の方言に node を足さずに** 他の約束（例: Creo の REST の `route`）を読むための規則。どの node 名でも、`request=` / `response=` という属性があれば種にする。Creo の `route "GET" "/api/memories" response="MemoriesResponse" { error status=409 response="ConflictBody" }` はそのまま効く。

種から、field の型が指す `message` / `enum` / `typedef` へ **到達で伝播** させる（`items=` / `keys=` / `values=` の中も含む）。応答の message の中で参照される message も応答になる。両方から到達する名前は両方の規則を適用する。

どこからも到達しない名前は向きが分からない。`--unknown-direction` に従う。

| 値 | 扱い |
|---|---|
| `both`（既定） | 要求・応答の両方の規則で厳しく見る |
| `skip` | 検査しない |

どちらでも、向きが分からない名前を警告 1 本にまとめて出す。

### 判定表

| 変更 | 要求（client → server） | 応答（server → client） |
|---|---|---|
| required の field が消えた | 通す（古い client は送り続けるが server は未知の key を無視する） | **壊れる** |
| 任意の field が消えた | 通す | 通す |
| field の型が変わった | **壊れる** | **壊れる** |
| required → 任意 | 通す | **壊れる** |
| 任意 → required | **壊れる** | 通す |
| 新しい required の field | **壊れる** | 通す |
| 任意の field / request / event / channel を足した | 通す | 通す |
| channel / request / event / message / enum / typedef が消えた | **壊れる** | **壊れる** |
| request が `returns` を持たなくなった | — | **壊れる** |
| typedef の `base_type` が変わった | **壊れる** | **壊れる** |
| enum の値が減った | **壊れる** | 通す |
| enum の値が増えた | 通す | **壊れる**（`--lenient-enum-additions` で警告） |

「通す」の前提は、server の検証（`SchemaRegistry::validate_request`）が **宣言された field だけを見て未知の key を素通しにする** こと。

応答の enum の値追加を既定で壊れる変更とする理由: 約束に enum と書くのは、client が fallback 無しの enum で decode するときだけ。寛容に読む client なら、約束の型を `string` にする（このとき値が増えても差分に出ない）。

### 型の比較

型は正規化した文字列で比べる（`string` / `int` / `array<Atlas>` / `map<string,Label>` / 宣言名）。`items=` 等を省略した `array` は `array<json>` になるので、要素型を後から書き足すことも型の変化として出る。

---

## Data Model

### 要素型の書き方（parser）

```kdl
field "atlases" type="array" items="Atlas" required=#true
field "labels" type="map" keys="string" values="Label"
```

- `items` → `FieldType::Array(T)`、`keys` / `values` → `FieldType::Map(K, V)`
- `keys` 省略時は `string`（JSON object のキーは常に文字列）、`items` / `values` 省略時は任意（`Json`）
- `array<T>` / `map<K, V>` という型名の構文は採らない。KDL の property で書く方が parser に小さく収まり、JSON Schema の語彙（`items` / `additionalProperties`）とも揃う

### 比較の単位

| 単位 | 中身 |
|---|---|
| channel | request（field と returns の field）、event（field）、event の向き |
| message | document 直下と protocol 直下の両方 |
| enum | 値の集合 |
| typedef | `base_type` |

方言に無い node は構造としては比べない（`request=` / `response=` を種として拾うだけ）。

---

## Implementation

- CLI: `unison schema-diff <old.kdl> <new.kdl> [--unknown-direction both|skip] [--lenient-enum-additions]`
- 出力: 警告は `!`、壊れる変更は `✗` で stderr に場所つきで並べる。壊れる変更があれば exit 1、無ければ `✓` を stdout に出して exit 0
- 未知 node の属性は `kdl` crate で生の文書を読んで拾う（`SchemaParser` は未知 node を捨てるため）
- テスト: `schema_diff.rs` の unit test（判定表の各行、向きの伝播、未知 node の種、enum の向き）

### 既知の制約

- runtime 検証（`SchemaRegistry::validate_request`）は要素型までは見ない。要素型を使うのは今のところ schema-diff と unison-mcp の tool schema
- field 名の変更は「消えた + 足した」として見る（rename の検出はしない）
- channel の `from` / `lifetime` / `backend` の変更は見ない
