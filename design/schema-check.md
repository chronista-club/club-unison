# schema-check — JSON が約束の型を満たすかを判定する

**Status**: Draft（実装済み、`unison schema-check`）
**Related**: [schema-diff.md](schema-diff.md)（約束どうしの比較）、[spec/02](../spec/02-unified-channel/SPEC.md) §4.1（型）、creo-memories spec 34（段 3: 今の server の実際の応答が配った client の約束を満たすか）
**対象**: `crates/unison-protocol/src/parser/check.rs`（`TypeIndex`）、`crates/unison-cli/src/schema_check.rs`

---

## Abstract

`schema-diff` は約束どうしを比べる。`schema-check` は **実際の JSON** を約束に当てる。

Creo の CI は各 route の実際の応答を作り、この CLI に渡すだけにする。判定の規則（null、int と float、enum、`items=` / `values=`、入れ子）を Creo 側に書くと、方言の解釈が Unison と Creo の 2 か所に分かれ、いずれ食い違う。規則は Unison の `TypeIndex` 1 か所に置く。

---

## Architecture

判定は **client が応答として decode するときに失敗する条件** を誤りにする。

| 約束 | 通す JSON | 誤り |
|---|---|---|
| `required=#true` の field | 値がある | 欠けている / null |
| 任意の field | 値がある / 欠けている / null | — |
| 知らない key | 通す | — |
| `string` | 文字列 | それ以外 |
| `int` | 整数（`3.0` のように小数部が 0 の数も通す） | `3.5` など小数部のある数、数以外 |
| `float` | 数（整数も通す） | 数以外 |
| `bool` | true / false | それ以外 |
| `json` | 何でも | — |
| `object` | object | それ以外 |
| `array`（`items="T"`） | 配列で、各要素が T | 配列以外 / 要素が T でない / 要素が null（T が `json` なら null も通す） |
| `map`（`values="V"`） | object で、各値が V | object 以外 / 値が V でない / 値が null（同上） |
| message 名 | object で、入れ子として同じ規則を満たす | — |
| enum 名 | 宣言された値のどれかの文字列 | それ以外 |
| typedef 名 | `base_type` で判定 | 循環して型に行き着かない |
| 宣言の無い型名（例: `bytes`） | — | **約束側の誤り**として報告（黙って通さない） |

`int` に `3.0` を通し `float` に整数を通すのは、どちらも数値としての decode が成り立つため。

### 名前で引ける形

`--message` に渡せる名前は次のとおり。

- top-level の `message`（document 直下 / protocol 直下）
- channel の中の `returns "X"` と `event "X"`（= client が decode する形。Unison 自身のスキーマはこちらで書く）

同名なら top-level の `message` を優先し、inline 同士の重複は先に書いた方を使う。

場所は JSONPath 風に出す。object の key は識別子なら `.key`、それ以外は `["key"]`、配列は `[i]`（例: `$.memories[1].fileRefs[0].id`、`$.labels["c d"]`）。

### 既存の検証との関係

`SchemaRegistry::validate_request` は server が **要求を受けたとき** の検証で、channel の request の field の直下だけを見る（要素型と入れ子は見ない）。`TypeIndex::check` は **応答を受ける側** の判定で、入れ子と要素型まで潜る。今は別の実装だが、`validate_request` を `TypeIndex` に寄せる余地がある。

---

## Implementation

### CLI

```bash
# 1 件
unison schema-check <contract.kdl> --message MemoriesResponse response.json
cat response.json | unison schema-check <contract.kdl> --message MemoriesResponse -

# まとめて (例: Creo の 21 の口を 1 回で)
unison schema-check <contract.kdl> --cases cases.json [--json]
```

`cases.json` の形:

```json
[
  { "name": "GET /api/memories", "message": "MemoriesResponse", "value": { "memories": [] } }
]
```

`name` は省略可で、出力の見出しにだけ使う（省くと message 名）。

### 出力

- 既定: 満たした組は `✓ <見出し>` を stdout、満たさない組は `✗ <見出し>: N error(s)` と場所つきの一覧を stderr
- `--json`: stdout に `{"ok": bool, "cases": N, "errors": [{"case", "path", "message"}]}`（`message` は誤りの文）
- 1 組でも満たさなければ exit 1

### テスト

- `crates/unison-protocol/tests/test_small_contract_check.rs` — 規則の表の各行、入れ子の場所、typedef の循環。実入力として Creo の約束の抜粋 `tests/fixtures/contracts/creo-ios-1.0.1-b11-excerpt.kdl` を使う
- `crates/unison-cli/src/schema_check.rs` の unit test — `--cases` の読み取りと `--json` の形
