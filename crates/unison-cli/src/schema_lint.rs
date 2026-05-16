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
use unison::parser::{ChannelBackend, ParsedSchema, SchemaParser};

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

    // 2. cross-channel invariant 検査
    let warnings = lint_invariants(&schema);

    if warnings.is_empty() {
        println!("✓ {}: ok", args.file.display());
        report_summary(&schema);
        Ok(())
    } else {
        eprintln!(
            "✗ {}: {} invariant violation(s)",
            args.file.display(),
            warnings.len()
        );
        for w in &warnings {
            eprintln!("  - {w}");
        }
        anyhow::bail!("schema-lint failed");
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
        if ch.backend() == ChannelBackend::Datagram {
            if let Some(id) = ch.channel_id {
                seen_ids.entry(id).or_default().push(ch.name.as_str());
            }
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
