//! `unison schema-lint <file.kdl>` — KDL schema を parse + invariant 検証。
//!
//! unison-protocol の `SchemaParser` で parse する (= KDL syntax error +
//! `Channel::validate()` の semantic check が走る)。それに加え、parser 単体では
//! 検出しない cross-channel な不変条件を CLI 側で追加検査する:
//!
//! - datagram channel 間の `channel_id` 衝突
//! - channel 名の重複
//! - `request`/`event` 名の channel 内重複
//! - backend が datagram なのに event を 1 つも持たない (= 無意味な channel)

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use unison::UnisonProtocol;
use unison::parser::{ChannelBackend, Field, FieldType, ParsedSchema, SchemaParser};

#[derive(Args)]
pub struct SchemaLintArgs {
    /// 検証対象の KDL schema ファイル
    pub file: PathBuf,
}

pub fn run(args: SchemaLintArgs) -> Result<()> {
    let src = std::fs::read_to_string(&args.file)
        .with_context(|| format!("failed to read {}", args.file.display()))?;

    // 1. parse (= KDL syntax + Channel::validate semantic check)
    let parser = SchemaParser::new();
    let schema = match parser.parse(&src) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("✗ {}: parse error", args.file.display());
            eprintln!("  {e}");
            anyhow::bail!("schema-lint failed");
        }
    };
    // load_schema 経由でも parse できることを確認 (= UnisonProtocol entry の sanity)
    let mut protocol = UnisonProtocol::new();
    if let Err(e) = protocol.load_schema(&src) {
        eprintln!("✗ {}: load_schema rejected", args.file.display());
        eprintln!("  {e}");
        anyhow::bail!("schema-lint failed");
    }

    // 2. cross-channel invariant 検査 (= 違反したら失敗)
    let violations = lint_invariants(&schema);

    // 3. 宣言のない型名 (= 警告。 exit code は変えない)
    let warnings = lint_unknown_field_types(&schema);

    if !violations.is_empty() {
        eprintln!(
            "✗ {}: {} invariant violation(s)",
            args.file.display(),
            violations.len()
        );
        for v in &violations {
            eprintln!("  - {v}");
        }
        print_warnings(&args.file, &warnings);
        anyhow::bail!("schema-lint failed");
    }

    println!("✓ {}: ok", args.file.display());
    print_warnings(&args.file, &warnings);
    report_summary(&schema);
    Ok(())
}

/// 警告を出す (= 失敗にはしない)。
fn print_warnings(file: &std::path::Path, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    eprintln!("! {}: {} warning(s)", file.display(), warnings.len());
    for w in warnings {
        eprintln!("  - {w}");
    }
}

/// parser 単体では検出しない cross-channel 不変条件を検査する。
fn lint_invariants(schema: &ParsedSchema) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(protocol) = &schema.protocol else {
        errs.push("no `protocol` block found".to_string());
        return errs;
    };

    // channel 名の重複
    let mut seen_names: std::collections::HashMap<&str, usize> = Default::default();
    for ch in &protocol.channels {
        *seen_names.entry(ch.name.as_str()).or_insert(0) += 1;
    }
    for (name, n) in &seen_names {
        if *n > 1 {
            errs.push(format!("channel name \"{name}\" declared {n} times"));
        }
    }

    // datagram channel_id 衝突
    let mut seen_ids: std::collections::HashMap<u64, Vec<&str>> = Default::default();
    for ch in &protocol.channels {
        if ch.backend() == ChannelBackend::Datagram
            && let Some(id) = ch.channel_id
        {
            seen_ids.entry(id).or_default().push(ch.name.as_str());
        }
    }
    for (id, owners) in &seen_ids {
        if owners.len() > 1 {
            errs.push(format!(
                "channel_id {id} collides across datagram channels: {}",
                owners.join(", ")
            ));
        }
    }

    // channel 内 request / event 名の重複、datagram なのに event 無し
    for ch in &protocol.channels {
        let mut req_names: std::collections::HashSet<&str> = Default::default();
        for r in &ch.requests {
            if !req_names.insert(r.name.as_str()) {
                errs.push(format!(
                    "channel \"{}\": duplicate request \"{}\"",
                    ch.name, r.name
                ));
            }
        }
        let mut ev_names: std::collections::HashSet<&str> = Default::default();
        for e in &ch.events {
            if !ev_names.insert(e.name.as_str()) {
                errs.push(format!(
                    "channel \"{}\": duplicate event \"{}\"",
                    ch.name, e.name
                ));
            }
        }
        if ch.backend() == ChannelBackend::Datagram && ch.events.is_empty() {
            errs.push(format!(
                "channel \"{}\": backend=\"datagram\" but declares no event \
                 (= unusable, datagram channels carry events only)",
                ch.name
            ));
        }
    }

    errs
}

/// 宣言されていない型名を使っている field を洗い出す (= 警告レベル)。
///
/// `Field::field_type()` は既知の型名に当てはまらないものを全部
/// [`FieldType::Custom`] にする。 Custom は下流で **完全に素通し**される:
/// `SchemaRegistry::validate_request` の型検査は `true` を返し、
/// `unison-mcp` が合成する JSON Schema にも型制約が付かない。
/// つまり `type="strng"` のような打ち間違いは、 そのフィールドの型検査を
/// 黙って無効化する。
///
/// Custom の正当な用途は `typedef` / `enum` で宣言した名前を参照することなので、
/// **宣言されていない名前だけ**を警告する。 これは invariant 違反 (= error) とは
/// 別枠で、 lint の exit code を変えない。
fn lint_unknown_field_types(schema: &ParsedSchema) -> Vec<String> {
    // typedef / enum で宣言された名前 = Custom として正当な参照先。
    // enum は document 直下と protocol 直下の両方に書ける。
    let mut declared: std::collections::HashSet<&str> = Default::default();
    for t in &schema.typedefs {
        declared.insert(t.name.as_str());
    }
    for e in &schema.enums {
        declared.insert(e.name.as_str());
    }
    if let Some(protocol) = &schema.protocol {
        for e in &protocol.enums {
            declared.insert(e.name.as_str());
        }
    }

    let mut warnings = Vec::new();
    let Some(protocol) = &schema.protocol else {
        return warnings;
    };

    for ch in &protocol.channels {
        for r in &ch.requests {
            let at = format!("channel \"{}\" request \"{}\"", ch.name, r.name);
            check_fields(&r.fields, &at, &declared, &mut warnings);
            if let Some(ret) = &r.returns {
                let at = format!("{at} returns \"{}\"", ret.name);
                check_fields(&ret.fields, &at, &declared, &mut warnings);
            }
        }
        for e in &ch.events {
            let at = format!("channel \"{}\" event \"{}\"", ch.name, e.name);
            check_fields(&e.fields, &at, &declared, &mut warnings);
        }
    }
    warnings
}

/// field 群を走査し、 宣言のない Custom 型に警告を積む。
fn check_fields(
    fields: &[Field],
    at: &str,
    declared: &std::collections::HashSet<&str>,
    warnings: &mut Vec<String>,
) {
    for f in fields {
        let FieldType::Custom(name) = f.field_type() else {
            continue;
        };
        if declared.contains(name.as_str()) {
            continue;
        }
        warnings.push(format!(
            "{at}: field \"{}\" has unknown type \"{name}\"{} \
             — 未知型は Custom 扱いになり、 型検査も JSON Schema の型制約も付かない",
            f.name,
            unknown_type_hint(&name),
        ));
    }
}

/// よくある間違いに対する直し方の示唆。
fn unknown_type_hint(name: &str) -> String {
    if name.starts_with("array<") || name.starts_with("map<") {
        // 下流の実 schema で使われている構文だが parser は未実装。
        // Array/Map にすらならないので「配列である」ことすら検証されない。
        " (typed-element 構文は未実装。 要素型を落として `array` / `map` と書くか、 \
         `typedef` で名前を付ける)"
            .to_string()
    } else {
        match name {
            "number" => " (JSON Schema の語彙。 Unison では `float` か `int`)".to_string(),
            "integer" => " (JSON Schema の語彙。 Unison では `int`)".to_string(),
            "boolean" => " (JSON Schema の語彙。 Unison では `bool`)".to_string(),
            "any" | "unknown" => " (任意 JSON は `json`)".to_string(),
            _ => " (typo でなければ `typedef` / `enum` で宣言する)".to_string(),
        }
    }
}

/// 検証成功時に schema の概要を出す。
fn report_summary(schema: &ParsedSchema) {
    let Some(protocol) = &schema.protocol else {
        return;
    };
    println!(
        "  protocol \"{}\" v{} — {} channel(s)",
        protocol.name,
        protocol.version,
        protocol.channels.len(),
    );
    for ch in &protocol.channels {
        let id = ch
            .channel_id
            .map(|i| format!(" channel_id={i}"))
            .unwrap_or_default();
        println!(
            "    - {} [backend={:?}{}] {} request(s), {} event(s)",
            ch.name,
            ch.backend(),
            id,
            ch.requests.len(),
            ch.events.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> ParsedSchema {
        SchemaParser::new().parse(src).expect("schema should parse")
    }

    /// 既知の型だけを使う schema は警告を出さない。
    #[test]
    fn known_field_types_produce_no_warning() {
        let schema = parse(
            r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "a" type="string"
            field "b" type="int"
            field "c" type="float"
            field "d" type="bool"
            field "e" type="json"
            field "f" type="object"
            field "g" type="array"
            field "h" type="map"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#,
        );
        assert_eq!(lint_unknown_field_types(&schema), Vec::<String>::new());
    }

    /// 綴り間違いの型は警告になる (= 今は Custom として黙って通り、型検査が消える)。
    #[test]
    fn misspelled_field_type_warns() {
        let schema = parse(
            r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "a" type="strng"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#,
        );
        let warnings = lint_unknown_field_types(&schema);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("strng"), "got: {warnings:?}");
        assert!(warnings[0].contains("\"a\""), "got: {warnings:?}");
    }

    /// `number` は JSON Schema の語彙であって Unison の型ではない。
    /// 下流の実 schema に 12 箇所あり、 現状すべて型検査を失っている。
    #[test]
    fn json_schema_vocabulary_warns_with_hint() {
        let schema = parse(
            r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "n" type="number"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#,
        );
        let warnings = lint_unknown_field_types(&schema);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(
            warnings[0].contains("float"),
            "`number` には `float` を薦めるべき: {warnings:?}"
        );
    }

    /// `array<T>` は未実装構文。 `Array` にすらならず、 配列であることも検証されない。
    #[test]
    fn typed_element_syntax_warns_as_unimplemented() {
        let schema = parse(
            r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "xs" type="array<string>"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#,
        );
        let warnings = lint_unknown_field_types(&schema);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(
            warnings[0].contains("array") && warnings[0].contains("未実装"),
            "got: {warnings:?}"
        );
    }

    /// typedef / enum で宣言された名前は Custom として正当なので警告しない。
    #[test]
    fn declared_typedef_and_enum_names_are_accepted() {
        let schema = parse(
            r#"
typedef "EntId" {
    base_type "string"
}
enum "Color" {
    values "red" "green"
}
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "R" {
            field "id" type="EntId"
            field "col" type="Color"
            returns "Res" {
                field "ok" type="bool"
            }
        }
    }
}
"#,
        );
        assert_eq!(lint_unknown_field_types(&schema), Vec::<String>::new());
    }

    /// event の field も走査対象。
    #[test]
    fn event_fields_are_checked_too() {
        let schema = parse(
            r#"
protocol "t" version="1.0.0" {
    channel "c" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
        event "E" {
            field "bad" type="flaot"
        }
    }
}
"#,
        );
        let warnings = lint_unknown_field_types(&schema);
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(warnings[0].contains("flaot"), "got: {warnings:?}");
    }
}
