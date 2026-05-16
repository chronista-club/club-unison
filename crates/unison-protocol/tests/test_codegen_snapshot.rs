//! Codegen 回帰スナップショット
//!
//! club-kdl-codegen への re-base 前の現行 generator 出力を golden fixture として
//! 凍結する。各スキーマについて Rust / TypeScript 両方を生成し、コミット済みの
//! fixture とバイト一致を検証する。re-base 後にパイプラインを差し替えた際、
//! 出力差分が即座にこのテストで surface する。
//!
//! fixture の更新: `UPDATE_CODEGEN_SNAPSHOTS=1` を立てて実行すると再生成する。
//! generator を意図的に変更したときのみ使うこと。
//!
//! fixture: `tests/fixtures/codegen/<schema>.{rs,ts}.txt`

use std::path::PathBuf;

use unison::codegen::CodeGenerator;
use unison::parser::TypeRegistry;
use unison::prelude::*;

/// スナップショット対象スキーマ。
/// (論理名, KDL ファイルへのワークスペース相対パス)
const SCHEMAS: &[(&str, &str)] = &[
    ("ping_pong", "../../schemas/ping_pong.kdl"),
    ("hierophant", "../../schemas/hierophant.kdl"),
    ("creo_sync", "tests/fixtures/creo_sync.kdl"),
];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    crate_root().join("tests/fixtures/codegen")
}

/// 1 スキーマを Rust / TS で生成し、(rust, typescript) を返す。
fn generate(schema_path: &str) -> (String, String) {
    let abs = crate_root().join(schema_path);
    let source = std::fs::read_to_string(&abs)
        .unwrap_or_else(|e| panic!("スキーマ読み込み失敗 {}: {e}", abs.display()));

    let parser = SchemaParser::new();
    let parsed = parser
        .parse(&source)
        .unwrap_or_else(|e| panic!("スキーマパース失敗 {schema_path}: {e}"));
    let type_registry = TypeRegistry::new();

    let rust = RustGenerator::new()
        .generate(&parsed, &type_registry)
        .unwrap_or_else(|e| panic!("Rust 生成失敗 {schema_path}: {e}"));
    let ts = TypeScriptGenerator::new()
        .generate(&parsed, &type_registry)
        .unwrap_or_else(|e| panic!("TypeScript 生成失敗 {schema_path}: {e}"));

    (rust, ts)
}

/// 1 つの fixture を検証 or 更新する。
fn check_or_update(name: &str, ext: &str, generated: &str) {
    let path = fixture_dir().join(format!("{name}.{ext}.txt"));

    if std::env::var("UPDATE_CODEGEN_SNAPSHOTS").is_ok() {
        std::fs::create_dir_all(fixture_dir()).unwrap();
        std::fs::write(&path, generated)
            .unwrap_or_else(|e| panic!("fixture 書き込み失敗 {}: {e}", path.display()));
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "fixture が見つからない {}: {e}\n\
             UPDATE_CODEGEN_SNAPSHOTS=1 で再生成してください",
            path.display()
        )
    });

    assert_eq!(
        expected,
        generated,
        "codegen 出力が fixture {} と一致しません。\n\
         generator を意図的に変更した場合は UPDATE_CODEGEN_SNAPSHOTS=1 で更新し、\n\
         差分をレビューしてください。",
        path.display()
    );
}

#[test]
fn snapshot_codegen_output() {
    for (name, schema_path) in SCHEMAS {
        let (rust, ts) = generate(schema_path);
        check_or_update(name, "rs", &rust);
        check_or_update(name, "ts", &ts);
    }
}
