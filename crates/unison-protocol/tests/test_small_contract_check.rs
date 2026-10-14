//! `parser::check` — JSON が約束 (KDL) の型を満たすかの判定。
//!
//! 判定は「client が応答として decode するときに失敗する条件」:
//! required の欠落 / null、 型違い、 要素型違い、 入れ子の message の違反、 enum に無い値。
//! 知らない key、 任意 field の欠落 / null は通す。

use serde_json::json;
use unison::parser::{SchemaParser, TypeIndex, Violation};

const CREO_EXCERPT: &str = include_str!("fixtures/contracts/creo-ios-1.0.1-b11-excerpt.kdl");

fn creo() -> TypeIndex {
    TypeIndex::new(
        &SchemaParser::new()
            .parse(CREO_EXCERPT)
            .expect("fixture parses"),
    )
}

fn index(src: &str) -> TypeIndex {
    TypeIndex::new(&SchemaParser::new().parse(src).expect("schema parses"))
}

fn paths(violations: &[Violation]) -> Vec<&str> {
    violations.iter().map(|v| v.path.as_str()).collect()
}

// ---- 実入力 (Creo の約束の抜粋) ----

#[test]
fn creo_memories_response_that_matches_passes() {
    let value = json!({
        "memories": [
            {
                "id": "mem_1",
                "content": "hello",
                "updated_at": "2026-10-14T00:00:00.000Z",
                "fileRefs": [{ "id": "fil_1" }],
                "unknown_key": 42
            },
            { "id": "mem_2", "content": "world", "contentType": null }
        ],
        "total": 2
    });
    let violations = creo().check("MemoriesResponse", &value);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn creo_memories_response_reports_nested_paths() {
    let value = json!({
        "memories": [
            { "id": "mem_1", "content": "ok" },
            { "id": "mem_2" },
            { "id": "mem_3", "content": "x", "fileRefs": [{ "added_at": "2026-10-14" }] }
        ]
    });
    let violations = creo().check("MemoriesResponse", &value);
    assert_eq!(
        paths(&violations),
        vec!["$.memories[1].content", "$.memories[2].fileRefs[0].id"],
        "{violations:?}"
    );
}

#[test]
fn creo_required_null_is_a_violation() {
    let value = json!({ "plans": [{ "plan": "loft", "maxMemories": null, "maxAtlas": 3 }] });
    let violations = creo().check("PlansResponse", &value);
    assert_eq!(paths(&violations), vec!["$.plans[0].maxMemories"]);
    assert!(violations[0].reason.contains("null"), "{violations:?}");
}

// ---- 型 ----

const SCALARS: &str = r#"
message "S" {
    field "s" type="string"
    field "i" type="int"
    field "f" type="float"
    field "b" type="bool"
    field "j" type="json"
    field "o" type="object"
}
"#;

#[test]
fn scalar_types_that_match_pass() {
    let value = json!({ "s": "x", "i": 3, "f": 1.5, "b": true, "j": [1, "a"], "o": { "k": 1 } });
    assert!(index(SCALARS).check("S", &value).is_empty());
}

#[test]
fn scalar_type_mismatches_are_reported() {
    let value = json!({ "s": 1, "i": "3", "f": "1.5", "b": "true", "o": [] });
    let violations = index(SCALARS).check("S", &value);
    assert_eq!(
        paths(&violations),
        vec!["$.s", "$.i", "$.f", "$.b", "$.o"],
        "{violations:?}"
    );
}

#[test]
fn int_rejects_fractions_but_accepts_integral_floats() {
    let idx = index(SCALARS);
    let violations = idx.check("S", &json!({ "i": 3.5 }));
    assert_eq!(paths(&violations), vec!["$.i"]);
    assert!(
        violations[0].reason.contains("3.5"),
        "値も添える: {violations:?}"
    );
    assert!(idx.check("S", &json!({ "i": 3.0 })).is_empty());
}

#[test]
fn float_accepts_integers() {
    assert!(index(SCALARS).check("S", &json!({ "f": 2 })).is_empty());
}

// ---- array / map の要素 ----

#[test]
fn array_items_are_checked_per_element() {
    let idx = index(
        r#"
message "A" {
    field "xs" type="array" items="int" required=#true
}
"#,
    );
    let violations = idx.check("A", &json!({ "xs": [1, "two", 3, null] }));
    assert_eq!(
        paths(&violations),
        vec!["$.xs[1]", "$.xs[3]"],
        "{violations:?}"
    );
}

#[test]
fn map_values_are_checked_per_entry() {
    let idx = index(
        r#"
message "M" {
    field "labels" type="map" values="string"
}
"#,
    );
    let violations = idx.check(
        "M",
        &json!({ "labels": { "a": "x", "b": 2, "c d": false } }),
    );
    assert_eq!(
        paths(&violations),
        vec![r#"$.labels.b"#, r#"$.labels["c d"]"#],
        "{violations:?}"
    );
}

#[test]
fn untyped_array_and_map_accept_any_elements() {
    let idx = index(
        r#"
message "U" {
    field "xs" type="array"
    field "m" type="map"
}
"#,
    );
    assert!(
        idx.check("U", &json!({ "xs": [1, "a", null], "m": { "k": [] } }))
            .is_empty()
    );
    assert_eq!(
        paths(&idx.check("U", &json!({ "xs": {}, "m": [] }))),
        vec!["$.xs", "$.m"]
    );
}

// ---- enum / typedef ----

#[test]
fn enum_accepts_only_declared_values() {
    let idx = creo();
    assert!(
        idx.check(
            "ReportRequest",
            &json!({ "memoryId": "m", "reason": "spam" })
        )
        .is_empty()
    );
    let violations = idx.check(
        "ReportRequest",
        &json!({ "memoryId": "m", "reason": "rude" }),
    );
    assert_eq!(paths(&violations), vec!["$.reason"]);
    assert!(violations[0].reason.contains("rude"), "{violations:?}");
}

#[test]
fn typedef_is_checked_by_its_base_type() {
    let idx = index(
        r#"
typedef "AtlasId" {
    base_type "string"
}
message "T" {
    field "id" type="AtlasId" required=#true
}
"#,
    );
    assert!(idx.check("T", &json!({ "id": "atl_1" })).is_empty());
    assert_eq!(paths(&idx.check("T", &json!({ "id": 1 }))), vec!["$.id"]);
}

// ---- 約束側の誤り ----

#[test]
fn undeclared_type_name_is_reported_not_passed() {
    let idx = index(
        r#"
message "X" {
    field "blob" type="bytes"
}
"#,
    );
    let violations = idx.check("X", &json!({ "blob": "AAAA" }));
    assert_eq!(paths(&violations), vec!["$.blob"]);
    assert!(violations[0].reason.contains("bytes"), "{violations:?}");
}

#[test]
fn unknown_message_name_is_reported_at_root() {
    let violations = creo().check("NoSuchMessage", &json!({}));
    assert_eq!(paths(&violations), vec!["$"]);
}

#[test]
fn message_value_must_be_an_object() {
    let violations = creo().check("MemoriesResponse", &json!([1, 2]));
    assert_eq!(paths(&violations), vec!["$"]);
}

#[test]
fn self_referencing_typedefs_do_not_loop_forever() {
    let idx = index(
        r#"
typedef "A" {
    base_type "B"
}
typedef "B" {
    base_type "A"
}
message "L" {
    field "x" type="A"
}
"#,
    );
    let violations = idx.check("L", &json!({ "x": 1 }));
    assert_eq!(paths(&violations), vec!["$.x"]);
}

// ---- Unison 自身のスキーマ (channel の中の returns / event) ----

const QUICKSTART: &str = r#"
protocol "quickstart" version="1.0.0" {
    channel "echo" from="client" lifetime="persistent" {
        request "Echo" {
            field "text" type="string" required=#true
            returns "EchoResp" {
                field "text" type="string" required=#true
            }
        }
    }
    channel "clock" from="client" lifetime="persistent" {
        event "Tick" {
            field "seq" type="int"
        }
    }
}
"#;

/// client が decode する形 (= `returns` と `event`) も名前で引ける。
#[test]
fn inline_returns_and_events_can_be_checked_by_name() {
    let idx = index(QUICKSTART);
    assert!(idx.check("EchoResp", &json!({ "text": "hi" })).is_empty());
    assert_eq!(paths(&idx.check("EchoResp", &json!({}))), vec!["$.text"]);
    assert_eq!(
        paths(&idx.check("Tick", &json!({ "seq": "x" }))),
        vec!["$.seq"]
    );
}

/// top-level の message が同名の inline の形より優先される。
#[test]
fn top_level_message_wins_over_inline_shape_with_same_name() {
    let src = format!(
        r#"
message "EchoResp" {{
    field "other" type="int" required=#true
}}
{QUICKSTART}"#
    );
    let idx = index(&src);
    assert_eq!(
        paths(&idx.check("EchoResp", &json!({ "text": "hi" }))),
        vec!["$.other"]
    );
}
