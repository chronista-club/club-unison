//! `unison schema-check <contract.kdl>` — JSON が約束の型を (応答として) 満たすか。
//!
//! 判定の規則は `unison::parser::TypeIndex` に 1 か所だけ置き、 CLI は入力と出力の形を
//! 持つだけにする。 約束を書く側 (例: Creo の REST の CI) は規則を持たず、 実際の応答を
//! この CLI に渡す。
//!
//! - 1 件: `unison schema-check <contract.kdl> --message M <file.json|->`
//! - まとめて: `unison schema-check <contract.kdl> --cases <cases.json>`
//!   (`[{"name": "GET /api/atlas", "message": "AtlasListResponse", "value": {...}}]`、
//!   `name` は省略可で、 出力の見出しにだけ使う)
//!
//! 約束を満たさない箇所があれば exit 1。 `--json` で機械が読める形を stdout に出す。

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use serde_json::{Value, json};
use unison::parser::{SchemaParser, TypeIndex, Violation};

#[derive(Args)]
pub struct SchemaCheckArgs {
    /// 約束の KDL schema
    pub contract: PathBuf,
    /// 入力の JSON を読む message 名 (`--cases` と排他)
    #[arg(long, conflicts_with = "cases", requires = "input")]
    pub message: Option<String>,
    /// 判定する JSON のファイル (`-` で stdin)
    pub input: Option<PathBuf>,
    /// 複数の組 (message と JSON) をまとめた JSON ファイル
    #[arg(long, conflicts_with = "input")]
    pub cases: Option<PathBuf>,
    /// 結果を JSON で stdout に出す
    #[arg(long)]
    pub json: bool,
}

/// 判定する 1 組。
#[derive(Debug)]
pub struct Case {
    /// 出力の見出し (= `name`、 無ければ message 名)
    pub label: String,
    pub message: String,
    pub value: Value,
}

/// 1 組の判定結果。
#[derive(Debug)]
pub struct CaseResult {
    pub label: String,
    pub violations: Vec<Violation>,
}

pub fn run(args: SchemaCheckArgs) -> Result<()> {
    let src = std::fs::read_to_string(&args.contract)
        .with_context(|| format!("failed to read {}", args.contract.display()))?;
    let schema = SchemaParser::new()
        .parse(&src)
        .map_err(|e| anyhow::anyhow!("{}: {e}", args.contract.display()))?;
    let index = TypeIndex::new(&schema);

    let cases = match (&args.message, &args.input, &args.cases) {
        (Some(message), Some(input), None) => vec![Case {
            label: message.clone(),
            message: message.clone(),
            value: read_json(input)?,
        }],
        (None, None, Some(path)) => parse_cases(&read_json(path)?)?,
        _ => anyhow::bail!("give either `--message <M> <file|->` or `--cases <file>`"),
    };

    let results = check_cases(&index, &cases);
    let failed = results.iter().filter(|r| !r.violations.is_empty()).count();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&render_json(&results))?);
    } else {
        for r in &results {
            if r.violations.is_empty() {
                println!("✓ {}", r.label);
            } else {
                eprintln!("✗ {}: {} error(s)", r.label, r.violations.len());
                for v in &r.violations {
                    eprintln!("  - {}: {}", v.path, v.reason);
                }
            }
        }
    }

    if failed > 0 {
        anyhow::bail!(
            "schema-check: {failed} of {} case(s) do not satisfy the contract",
            results.len()
        );
    }
    Ok(())
}

/// ファイル (`-` なら stdin) を JSON として読む。
fn read_json(path: &PathBuf) -> Result<Value> {
    let text = if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("failed to read stdin")?;
        buf
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?
    };
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// `--cases` の JSON を組の並びにする。
pub fn parse_cases(value: &Value) -> Result<Vec<Case>> {
    let items = value
        .as_array()
        .context("--cases must be a JSON array of {\"name\"?, \"message\", \"value\"}")?;
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let message = item
                .get("message")
                .and_then(Value::as_str)
                .with_context(|| format!("--cases[{i}]: \"message\" (string) is required"))?
                .to_string();
            let value = item
                .get("value")
                .cloned()
                .with_context(|| format!("--cases[{i}]: \"value\" is required"))?;
            let label = item
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| message.clone(), str::to_string);
            Ok(Case {
                label,
                message,
                value,
            })
        })
        .collect()
}

/// 各組を判定する。
pub fn check_cases(index: &TypeIndex, cases: &[Case]) -> Vec<CaseResult> {
    cases
        .iter()
        .map(|c| CaseResult {
            label: c.label.clone(),
            violations: index.check(&c.message, &c.value),
        })
        .collect()
}

/// `--json` の出力形 (= `{"ok", "cases", "errors": [{"case", "path", "message"}]}`)。
pub fn render_json(results: &[CaseResult]) -> Value {
    let errors: Vec<Value> = results
        .iter()
        .flat_map(|r| {
            r.violations.iter().map(|v| {
                json!({
                    "case": r.label,
                    "path": v.path,
                    "message": v.reason,
                })
            })
        })
        .collect();
    json!({
        "ok": errors.is_empty(),
        "cases": results.len(),
        "errors": errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACT: &str = r#"
message "Atlas" {
    field "id" type="string" required=#true
    field "name" type="string" required=#true
}
message "AtlasListResponse" {
    field "atlas" type="array" items="Atlas" required=#true
}
"#;

    fn index() -> TypeIndex {
        TypeIndex::new(&SchemaParser::new().parse(CONTRACT).unwrap())
    }

    #[test]
    fn parse_cases_reads_name_message_and_value() {
        let cases = parse_cases(&json!([
            { "name": "GET /api/atlas", "message": "AtlasListResponse", "value": { "atlas": [] } },
            { "message": "Atlas", "value": { "id": "a" } }
        ]))
        .unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].label, "GET /api/atlas");
        assert_eq!(cases[0].message, "AtlasListResponse");
        // name を省くと message 名が見出しになる
        assert_eq!(cases[1].label, "Atlas");
        assert_eq!(cases[1].value, json!({ "id": "a" }));
    }

    #[test]
    fn parse_cases_rejects_malformed_input() {
        assert!(
            parse_cases(&json!({ "message": "Atlas" })).is_err(),
            "not an array"
        );
        assert!(
            parse_cases(&json!([{ "value": {} }])).is_err(),
            "message is required"
        );
        assert!(
            parse_cases(&json!([{ "message": "Atlas" }])).is_err(),
            "value is required"
        );
    }

    #[test]
    fn check_cases_reports_violations_per_case() {
        let cases = parse_cases(&json!([
            { "name": "ok", "message": "AtlasListResponse", "value": { "atlas": [{ "id": "a", "name": "A" }] } },
            { "name": "ng", "message": "AtlasListResponse", "value": { "atlas": [{ "id": "a" }] } }
        ]))
        .unwrap();
        let results = check_cases(&index(), &cases);
        assert_eq!(results.len(), 2);
        assert!(results[0].violations.is_empty(), "{results:?}");
        assert_eq!(results[1].violations[0].path, "$.atlas[0].name");
    }

    #[test]
    fn render_json_has_ok_flag_and_flat_errors() {
        let cases = parse_cases(&json!([
            { "name": "ng", "message": "AtlasListResponse", "value": { "atlas": [{ "id": 1 }] } }
        ]))
        .unwrap();
        let out = render_json(&check_cases(&index(), &cases));
        assert_eq!(out["ok"], json!(false));
        assert_eq!(out["cases"], json!(1));
        let errors = out["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 2, "{out}");
        assert_eq!(errors[0]["case"], json!("ng"));
        assert_eq!(errors[0]["path"], json!("$.atlas[0].id"));
        assert!(
            errors[0]["message"].as_str().unwrap().contains("string"),
            "{out}"
        );
    }

    #[test]
    fn render_json_ok_when_no_violations() {
        let cases = parse_cases(&json!([
            { "message": "Atlas", "value": { "id": "a", "name": "A" } }
        ]))
        .unwrap();
        let out = render_json(&check_cases(&index(), &cases));
        assert_eq!(out, json!({ "ok": true, "cases": 1, "errors": [] }));
    }
}
