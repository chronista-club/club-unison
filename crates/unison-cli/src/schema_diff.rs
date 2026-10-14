//! `unison schema-diff <old.kdl> <new.kdl>` — 旧版と比べて壊れる変更を見つける。
//!
//! 「配った client が前提にしている約束 (= old)」を「今の server の約束 (= new)」が
//! 壊していないかを CI で確かめるための道具。 壊れる変更があれば exit 1。
//!
//! 判定は **向き** で逆になる:
//!
//! - **要求** (client → server): 古い client は新しく required になった field を送れない
//! - **応答** (server → client): 古い client は required だった field を当てにしている
//!
//! 向きの種は 2 系統から取る:
//!
//! - Unison の構造: `request` の field = 要求、 `returns` の field = 応答、
//!   `event` の field = channel の `from` で決まる (`server` なら応答)
//! - 未知の top-level node の `request="X"` / `response="Y"` 属性
//!   (= 方言に無い node でも、 X を要求・Y を応答の種として扱う。 例: Creo の `route`)
//!
//! 種から field の型が指す `message` / `enum` へ到達で伝播させる。 どこからも
//! 到達しない名前は向きが分からないので `--unknown-direction` に従う。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use unison::parser::{ChannelFrom, Field, FieldType, SchemaParser};

#[derive(Args)]
pub struct SchemaDiffArgs {
    /// 旧版 (= 配った client が前提にしている約束) の KDL schema
    pub old: PathBuf,
    /// 新版 (= 今の server の約束) の KDL schema
    pub new: PathBuf,
    /// どこからも参照されず向きが分からない message / enum の扱い
    #[arg(long, value_enum, default_value_t = UnknownDirection::Both)]
    pub unknown_direction: UnknownDirection,
    /// 応答側の enum への値の追加を、 壊れる変更ではなく警告にする
    ///
    /// 既定では壊れる変更。 約束に enum と書くのは client が fallback 無しの enum で
    /// 読むときだけで (寛容に読むなら約束を `type="string"` にする)、 そのとき未知の値は
    /// decode に失敗するため。
    #[arg(long)]
    pub lenient_enum_additions: bool,
}

/// 向きが分からない名前の扱い。
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum UnknownDirection {
    /// 要求・応答の両方の規則で厳しく見る
    Both,
    /// 検査しない (= 警告だけ出す)
    Skip,
}

/// 判定の設定。
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub unknown_direction: UnknownDirection,
    pub lenient_enum_additions: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            unknown_direction: UnknownDirection::Both,
            lenient_enum_additions: false,
        }
    }
}

/// 差分検査の結果。
#[derive(Debug, Default)]
pub struct Report {
    /// 壊れる変更 (= 1 つでもあれば exit 1)
    pub breaking: Vec<String>,
    /// 注意 (= exit code は変えない)
    pub warnings: Vec<String>,
}

pub fn run(args: SchemaDiffArgs) -> Result<()> {
    let read = |p: &PathBuf| {
        std::fs::read_to_string(p).with_context(|| format!("failed to read {}", p.display()))
    };
    let old_src = read(&args.old)?;
    let new_src = read(&args.new)?;
    let opts = Options {
        unknown_direction: args.unknown_direction,
        lenient_enum_additions: args.lenient_enum_additions,
    };
    let report = diff(&old_src, &new_src, &opts)?;
    let label = format!("{} → {}", args.old.display(), args.new.display());

    if !report.warnings.is_empty() {
        eprintln!("! {label}: {} warning(s)", report.warnings.len());
        for w in &report.warnings {
            eprintln!("  - {w}");
        }
    }
    if !report.breaking.is_empty() {
        eprintln!("✗ {label}: {} breaking change(s)", report.breaking.len());
        for b in &report.breaking {
            eprintln!("  - {b}");
        }
        anyhow::bail!("schema-diff found breaking changes");
    }
    println!("✓ {label}: no breaking changes");
    Ok(())
}

/// 旧版と新版の KDL を比べる。
///
/// 向きは **旧版** の参照関係から決める (= 配った client が前提にしている形)。
pub fn diff(old_src: &str, new_src: &str, opts: &Options) -> Result<Report> {
    let old = Contract::parse(old_src).context("failed to parse old schema")?;
    let new = Contract::parse(new_src).context("failed to parse new schema")?;
    let dirs = old.directions();
    let mut report = Report::default();

    diff_channels(&old, &new, &mut report.breaking);

    // message / enum は参照から決まった向きで見る。 向きが分からない名前は
    // `--unknown-direction` に従い、 まとめて 1 本の警告にする。
    let mut unknown = Vec::new();
    let mut resolve = |name: &str| match dirs.get(name).copied().filter(|d| d.is_known()) {
        Some(d) => Some(d),
        None => {
            unknown.push(format!("\"{name}\""));
            match opts.unknown_direction {
                UnknownDirection::Both => Some(Dir::BOTH),
                UnknownDirection::Skip => None,
            }
        }
    };

    for (name, old_fields) in &old.messages {
        let Some(dir) = resolve(name) else { continue };
        let at = format!("message \"{name}\"");
        match new.messages.get(name) {
            Some(new_fields) => {
                compare_fields(&at, old_fields, new_fields, dir, &mut report.breaking)
            }
            None => report.breaking.push(format!("{at} was removed")),
        }
    }

    for (name, old_values) in &old.enums {
        let Some(dir) = resolve(name) else { continue };
        let at = format!("enum \"{name}\" ({})", dir.label());
        let Some(new_values) = new.enums.get(name) else {
            report.breaking.push(format!("enum \"{name}\" was removed"));
            continue;
        };
        if dir.req {
            for v in old_values.difference(new_values) {
                report.breaking.push(format!(
                    "{at}: value \"{v}\" was removed — old clients may still send it"
                ));
            }
        }
        if dir.resp {
            for v in new_values.difference(old_values) {
                let msg =
                    format!("{at}: value \"{v}\" was added — old clients may fail to decode it");
                if opts.lenient_enum_additions {
                    report.warnings.push(msg);
                } else {
                    report.breaking.push(msg);
                }
            }
        }
    }

    for (name, old_base) in &old.typedefs {
        match new.typedefs.get(name) {
            None => report
                .breaking
                .push(format!("typedef \"{name}\" was removed")),
            Some(new_base) if new_base != old_base => report.breaking.push(format!(
                "typedef \"{name}\": base_type changed {old_base} → {new_base}"
            )),
            Some(_) => {}
        }
    }

    if !unknown.is_empty() {
        let how = match opts.unknown_direction {
            UnknownDirection::Both => "checked with both request and response rules",
            UnknownDirection::Skip => "skipped",
        };
        report.warnings.push(format!(
            "direction unknown for {} (not referenced from any request / returns / event \
             or request= / response= attribute); {how}",
            unknown.join(", ")
        ));
    }

    Ok(report)
}

/// channel / request / event の消失と、 inline field の差分。
fn diff_channels(old: &Contract, new: &Contract, breaking: &mut Vec<String>) {
    for (cname, old_ch) in &old.channels {
        let Some(new_ch) = new.channels.get(cname) else {
            breaking.push(format!("channel \"{cname}\" was removed"));
            continue;
        };
        for (rname, old_req) in &old_ch.requests {
            let at = format!("channel \"{cname}\" request \"{rname}\"");
            let Some(new_req) = new_ch.requests.get(rname) else {
                breaking.push(format!("{at} was removed"));
                continue;
            };
            compare_fields(&at, &old_req.fields, &new_req.fields, Dir::REQ, breaking);
            match (&old_req.returns, &new_req.returns) {
                (Some(o), Some(n)) => {
                    compare_fields(&format!("{at} returns"), o, n, Dir::RESP, breaking)
                }
                (Some(_), None) => breaking.push(format!("{at} no longer returns a response")),
                _ => {}
            }
        }
        for (ename, old_fields) in &old_ch.events {
            let at = format!("channel \"{cname}\" event \"{ename}\"");
            match new_ch.events.get(ename) {
                Some(new_fields) => {
                    compare_fields(&at, old_fields, new_fields, old_ch.event_dir, breaking)
                }
                None => breaking.push(format!("{at} was removed")),
            }
        }
    }
}

/// 1 つの形 (= field の並び) を向きの規則で比べる。
fn compare_fields(at: &str, old: &[Field], new: &[Field], dir: Dir, breaking: &mut Vec<String>) {
    let label = dir.label();
    let new_by_name: BTreeMap<&str, &Field> = new.iter().map(|f| (f.name.as_str(), f)).collect();
    let old_names: BTreeSet<&str> = old.iter().map(|f| f.name.as_str()).collect();

    for f in old {
        let Some(g) = new_by_name.get(f.name.as_str()) else {
            if dir.resp && f.required {
                breaking.push(format!(
                    "{at} ({label}): required field \"{}\" was removed — old clients expect it",
                    f.name
                ));
            }
            continue;
        };
        let (old_ty, new_ty) = (type_key(&f.field_type()), type_key(&g.field_type()));
        if old_ty != new_ty {
            breaking.push(format!(
                "{at} ({label}): field \"{}\" changed type {old_ty} → {new_ty}",
                f.name
            ));
        }
        if dir.resp && f.required && !g.required {
            breaking.push(format!(
                "{at} ({label}): field \"{}\" is no longer required — old clients expect it",
                f.name
            ));
        }
        if dir.req && !f.required && g.required {
            breaking.push(format!(
                "{at} ({label}): field \"{}\" became required — old clients do not send it",
                f.name
            ));
        }
    }
    for g in new {
        if dir.req && g.required && !old_names.contains(g.name.as_str()) {
            breaking.push(format!(
                "{at} ({label}): new required field \"{}\" — old clients do not send it",
                g.name
            ));
        }
    }
}

/// 型を比較・表示用の文字列にする (= `array<Atlas>` / `map<string,Label>`)。
fn type_key(t: &FieldType) -> String {
    match t {
        FieldType::String => "string".into(),
        FieldType::Int => "int".into(),
        FieldType::Float => "float".into(),
        FieldType::Bool => "bool".into(),
        FieldType::Json => "json".into(),
        FieldType::Object => "object".into(),
        FieldType::Array(e) => format!("array<{}>", type_key(e)),
        FieldType::Map(k, v) => format!("map<{},{}>", type_key(k), type_key(v)),
        FieldType::Enum(vs) => format!("enum({})", vs.join("|")),
        FieldType::Custom(n) => n.clone(),
    }
}

/// 型が参照する名前 (= `message` / `enum` / `typedef` の候補)。
fn referenced_names(t: &FieldType, out: &mut Vec<String>) {
    match t {
        FieldType::Custom(n) => out.push(n.clone()),
        FieldType::Array(e) => referenced_names(e, out),
        FieldType::Map(k, v) => {
            referenced_names(k, out);
            referenced_names(v, out);
        }
        _ => {}
    }
}

/// 向き (= 要求 / 応答 / 両方)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Dir {
    req: bool,
    resp: bool,
}

impl Dir {
    const REQ: Self = Self {
        req: true,
        resp: false,
    };
    const RESP: Self = Self {
        req: false,
        resp: true,
    };
    const BOTH: Self = Self {
        req: true,
        resp: true,
    };

    fn is_known(self) -> bool {
        self.req || self.resp
    }

    fn union(self, other: Self) -> Self {
        Self {
            req: self.req || other.req,
            resp: self.resp || other.resp,
        }
    }

    fn label(self) -> &'static str {
        match (self.req, self.resp) {
            (true, false) => "request",
            (false, true) => "response",
            _ => "request+response",
        }
    }
}

/// 1 つの schema を比べやすい形にしたもの。
struct Contract {
    channels: BTreeMap<String, ChannelShape>,
    messages: BTreeMap<String, Vec<Field>>,
    enums: BTreeMap<String, BTreeSet<String>>,
    typedefs: BTreeMap<String, String>,
    /// 方言に無い top-level node の `request=` / `response=` から取った向きの種
    seeds: Vec<(String, Dir)>,
}

struct ChannelShape {
    event_dir: Dir,
    requests: BTreeMap<String, RequestShape>,
    events: BTreeMap<String, Vec<Field>>,
}

struct RequestShape {
    fields: Vec<Field>,
    returns: Option<Vec<Field>>,
}

/// 未知 node とその子孫から `request=` / `response=` を拾う
/// (= 例: Creo の `route ... response="X" { error status=409 response="Y" }`)。
fn collect_seeds(node: &kdl::KdlNode, seeds: &mut Vec<(String, Dir)>) {
    for (key, dir) in [("request", Dir::REQ), ("response", Dir::RESP)] {
        if let Some(name) = node.get(key).and_then(|v| v.as_string()) {
            seeds.push((name.to_string(), dir));
        }
    }
    for child in node.iter_children() {
        collect_seeds(child, seeds);
    }
}

/// Unison の方言で意味を持つ top-level node。 これ以外は未知 node として扱う。
const KNOWN_TOP_LEVEL: &[&str] = &["protocol", "import", "message", "enum", "typedef"];

impl Contract {
    fn parse(src: &str) -> Result<Self> {
        let schema = SchemaParser::new()
            .parse(src)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let doc: kdl::KdlDocument = src.parse().context("KDL syntax error")?;

        let mut messages = BTreeMap::new();
        let mut enums = BTreeMap::new();
        let mut channels = BTreeMap::new();
        let protocol = schema.protocol.as_ref();
        let all_messages = schema
            .messages
            .iter()
            .chain(protocol.into_iter().flat_map(|p| &p.messages));
        for m in all_messages {
            messages.insert(m.name.clone(), m.fields.clone());
        }
        let all_enums = schema
            .enums
            .iter()
            .chain(protocol.into_iter().flat_map(|p| &p.enums));
        for e in all_enums {
            enums.insert(e.name.clone(), e.values.iter().cloned().collect());
        }
        for ch in protocol.into_iter().flat_map(|p| &p.channels) {
            let event_dir = match ch.from {
                ChannelFrom::Server => Dir::RESP,
                ChannelFrom::Client => Dir::REQ,
                ChannelFrom::Either => Dir::BOTH,
            };
            let requests = ch
                .requests
                .iter()
                .map(|r| {
                    let shape = RequestShape {
                        fields: r.fields.clone(),
                        returns: r.returns.as_ref().map(|ret| ret.fields.clone()),
                    };
                    (r.name.clone(), shape)
                })
                .collect();
            let events = ch
                .events
                .iter()
                .map(|e| (e.name.clone(), e.fields.clone()))
                .collect();
            channels.insert(
                ch.name.clone(),
                ChannelShape {
                    event_dir,
                    requests,
                    events,
                },
            );
        }
        let typedefs = schema
            .typedefs
            .iter()
            .map(|t| (t.name.clone(), t.base_type.clone()))
            .collect();

        let mut seeds = Vec::new();
        for node in doc.nodes() {
            if !KNOWN_TOP_LEVEL.contains(&node.name().value()) {
                collect_seeds(node, &mut seeds);
            }
        }

        Ok(Self {
            channels,
            messages,
            enums,
            typedefs,
            seeds,
        })
    }

    /// 種から field の型参照をたどり、 名前ごとの向きを決める。
    fn directions(&self) -> BTreeMap<String, Dir> {
        let mut queue: Vec<(String, Dir)> = self.seeds.clone();
        let push_refs = |fields: &[Field], dir: Dir, queue: &mut Vec<(String, Dir)>| {
            let mut names = Vec::new();
            for f in fields {
                referenced_names(&f.field_type(), &mut names);
            }
            queue.extend(names.into_iter().map(|n| (n, dir)));
        };
        for ch in self.channels.values() {
            for r in ch.requests.values() {
                push_refs(&r.fields, Dir::REQ, &mut queue);
                if let Some(ret) = &r.returns {
                    push_refs(ret, Dir::RESP, &mut queue);
                }
            }
            for fields in ch.events.values() {
                push_refs(fields, ch.event_dir, &mut queue);
            }
        }

        let mut dirs: BTreeMap<String, Dir> = BTreeMap::new();
        while let Some((name, dir)) = queue.pop() {
            let current = dirs.entry(name.clone()).or_default();
            let merged = current.union(dir);
            if merged == *current {
                continue;
            }
            *current = merged;
            if let Some(fields) = self.messages.get(&name) {
                push_refs(fields, merged, &mut queue);
            }
        }
        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(old: &str, new: &str) -> Report {
        diff(old, new, &Options::default()).expect("both schemas should parse")
    }

    fn check_with(old: &str, new: &str, opts: Options) -> Report {
        diff(old, new, &opts).expect("both schemas should parse")
    }

    /// request の field と returns の field を差し替えた 1 channel の schema。
    fn channel(req_fields: &str, ret_fields: &str) -> String {
        format!(
            r#"
protocol "t" version="1.0.0" {{
    channel "c" from="client" lifetime="persistent" {{
        request "Get" {{
            {req_fields}
            returns "GetRes" {{
                {ret_fields}
            }}
        }}
    }}
}}
"#
        )
    }

    fn assert_breaking(report: &Report, needle: &str) {
        assert!(
            report.breaking.iter().any(|b| b.contains(needle)),
            "expected a breaking change mentioning {needle:?}, got {report:?}"
        );
    }

    fn assert_clean(report: &Report) {
        assert!(
            report.breaking.is_empty(),
            "expected no breaking change, got {report:?}"
        );
    }

    // ---- 変わらない / 足すだけ ----

    #[test]
    fn identical_schemas_are_clean() {
        let s = channel(
            r#"field "id" type="string" required=#true"#,
            r#"field "name" type="string" required=#true"#,
        );
        let report = check(&s, &s);
        assert_clean(&report);
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    #[test]
    fn adding_optional_fields_and_methods_is_clean() {
        let old = channel(
            r#"field "id" type="string" required=#true"#,
            r#"field "name" type="string" required=#true"#,
        );
        let new = r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "Get" {
            field "id" type="string" required=#true
            field "verbose" type="bool"
            returns "GetRes" {
                field "name" type="string" required=#true
                field "extra" type="string"
            }
        }
        request "List" {
            returns "ListRes" {
                field "n" type="int"
            }
        }
    }
}
"#;
        assert_clean(&check(&old, new));
    }

    // ---- 応答 (server → client) ----

    #[test]
    fn response_required_field_removed_is_breaking() {
        let old = channel("", r#"field "name" type="string" required=#true"#);
        let new = channel("", r#"field "other" type="string""#);
        assert_breaking(&check(&old, &new), "\"name\"");
    }

    #[test]
    fn response_optional_field_removed_is_clean() {
        let old = channel("", r#"field "name" type="string""#);
        let new = channel("", "");
        assert_clean(&check(&old, &new));
    }

    #[test]
    fn response_required_becoming_optional_is_breaking() {
        let old = channel("", r#"field "name" type="string" required=#true"#);
        let new = channel("", r#"field "name" type="string""#);
        assert_breaking(&check(&old, &new), "\"name\"");
    }

    #[test]
    fn response_new_required_field_is_clean() {
        let old = channel("", "");
        let new = channel("", r#"field "name" type="string" required=#true"#);
        assert_clean(&check(&old, &new));
    }

    // ---- 要求 (client → server) ----

    #[test]
    fn request_new_required_field_is_breaking() {
        let old = channel("", "");
        let new = channel(r#"field "tenant" type="string" required=#true"#, "");
        assert_breaking(&check(&old, &new), "\"tenant\"");
    }

    #[test]
    fn request_optional_becoming_required_is_breaking() {
        let old = channel(r#"field "tenant" type="string""#, "");
        let new = channel(r#"field "tenant" type="string" required=#true"#, "");
        assert_breaking(&check(&old, &new), "\"tenant\"");
    }

    #[test]
    fn request_required_field_removed_is_clean() {
        let old = channel(r#"field "id" type="string" required=#true"#, "");
        let new = channel("", "");
        assert_clean(&check(&old, &new));
    }

    #[test]
    fn request_required_becoming_optional_is_clean() {
        let old = channel(r#"field "id" type="string" required=#true"#, "");
        let new = channel(r#"field "id" type="string""#, "");
        assert_clean(&check(&old, &new));
    }

    // ---- 型 (両方向) ----

    #[test]
    fn type_change_is_breaking_in_both_directions() {
        let old = channel(r#"field "id" type="string""#, r#"field "n" type="int""#);
        let new = channel(r#"field "id" type="int""#, r#"field "n" type="string""#);
        let report = check(&old, &new);
        assert_breaking(&report, "\"id\"");
        assert_breaking(&report, "\"n\"");
    }

    #[test]
    fn array_element_type_change_is_breaking() {
        let old = channel("", r#"field "xs" type="array" items="string""#);
        let new = channel("", r#"field "xs" type="array" items="int""#);
        assert_breaking(&check(&old, &new), "\"xs\"");
    }

    #[test]
    fn map_value_type_change_is_breaking() {
        let old = channel("", r#"field "m" type="map" values="string""#);
        let new = channel("", r#"field "m" type="map" values="int""#);
        assert_breaking(&check(&old, &new), "\"m\"");
    }

    // ---- 消えた ----

    #[test]
    fn removed_request_is_breaking() {
        let old = channel("", "");
        let new = r#"
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
    }
}
"#;
        assert_breaking(&check(&old, new), "\"Get\"");
    }

    #[test]
    fn removed_channel_is_breaking() {
        let old = channel("", "");
        let new = r#"
protocol "t" version="1.0.0" {
}
"#;
        assert_breaking(&check(&old, new), "\"c\"");
    }

    #[test]
    fn removed_event_is_breaking() {
        let old = r#"
protocol "t" version="1.0.0" {
    channel "feed" from="server" lifetime="persistent" {
        event "Tick" {
            field "seq" type="int" required=#true
        }
    }
}
"#;
        let new = r#"
protocol "t" version="1.0.0" {
    channel "feed" from="server" lifetime="persistent" {
    }
}
"#;
        assert_breaking(&check(old, new), "\"Tick\"");
    }

    #[test]
    fn removed_message_is_breaking() {
        let old = r#"
message "Atlas" {
    field "id" type="string" required=#true
}
"#;
        let new = "";
        assert_breaking(&check(old, new), "\"Atlas\"");
    }

    #[test]
    fn typedef_base_type_change_is_breaking() {
        let old = r#"
typedef "AtlasId" {
    base_type "string"
}
"#;
        let new = r#"
typedef "AtlasId" {
    base_type "int"
}
"#;
        assert_breaking(&check(old, new), "\"AtlasId\"");
    }

    // ---- event の向きは channel の from で決まる ----

    #[test]
    fn server_event_follows_response_rules() {
        let event = |fields: &str| {
            format!(
                r#"
protocol "t" version="1.0.0" {{
    channel "feed" from="server" lifetime="persistent" {{
        event "Tick" {{
            {fields}
        }}
    }}
}}
"#
            )
        };
        // 応答の規則: required の削除は壊れる、 required の追加は通す
        let old = event(r#"field "seq" type="int" required=#true"#);
        assert_breaking(&check(&old, &event("")), "\"seq\"");
        assert_clean(&check(&event(""), &old));
    }

    // ---- message への伝播 ----

    #[test]
    fn message_referenced_from_returns_follows_response_rules() {
        let old = r#"
message "Atlas" {
    field "name" type="string" required=#true
}
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "Get" {
            returns "GetRes" {
                field "atlases" type="array" items="Atlas"
            }
        }
    }
}
"#;
        let new = old.replace(r#"field "name" type="string" required=#true"#, "");
        let report = check(old, &new);
        assert_breaking(&report, "\"Atlas\"");
        // 向きが分かっているので「向き不明」の警告は出ない
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    #[test]
    fn nested_message_inherits_direction() {
        // Atlas (応答) → Owner も応答。 Owner に required を足しても壊れない
        let old = r#"
message "Owner" {
    field "id" type="string" required=#true
}
message "Atlas" {
    field "owner" type="Owner" required=#true
}
protocol "t" version="1.0.0" {
    channel "c" from="client" lifetime="persistent" {
        request "Get" {
            returns "GetRes" {
                field "atlas" type="Atlas" required=#true
            }
        }
    }
}
"#;
        let new = old.replace(
            r#"field "id" type="string" required=#true"#,
            r#"field "id" type="string" required=#true
    field "email" type="string" required=#true"#,
        );
        let report = check(old, &new);
        assert_clean(&report);
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    // ---- 未知 node (例: Creo の route) の request= / response= ----

    #[test]
    fn unknown_node_request_and_response_attributes_seed_direction() {
        let old = r#"
route "POST" "/api/memories" request="CreateMemory" response="Memory" {
    auth "bearer"
}
message "CreateMemory" {
    field "body" type="string" required=#true
}
message "Memory" {
    field "id" type="string" required=#true
}
"#;
        // 要求側に required を足す → 壊れる
        let new = old.replace(
            r#"field "body" type="string" required=#true"#,
            r#"field "body" type="string" required=#true
    field "atlas" type="string" required=#true"#,
        );
        assert_breaking(&check(old, &new), "\"atlas\"");

        // 応答側に required を足す → 壊れない
        let new = old.replace(
            r#"field "id" type="string" required=#true"#,
            r#"field "id" type="string" required=#true
    field "created_at" type="string" required=#true"#,
        );
        let report = check(old, &new);
        assert_clean(&report);
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    /// 未知 node の子 (例: route の中の error) の `response=` も向きの種にする。
    #[test]
    fn nested_unknown_node_attributes_seed_direction() {
        let old = r#"
route "DELETE" "/api/atlas/:id" {
    error status=409 response="ConflictBody"
}
message "ConflictBody" {
    field "reason" type="string" required=#true
}
"#;
        // 応答側に required を足しても壊れず、 向き不明の警告も出ない
        let new = old.replace(
            r#"field "reason" type="string" required=#true"#,
            r#"field "reason" type="string" required=#true
    field "count" type="int" required=#true"#,
        );
        let report = check(old, &new);
        assert_clean(&report);
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    // ---- 向きが分からない名前 ----

    #[test]
    fn unreached_message_is_checked_both_ways_by_default() {
        let old = r#"
message "Loose" {
    field "a" type="string" required=#true
}
"#;
        let new = r#"
message "Loose" {
    field "a" type="string"
    field "b" type="string" required=#true
}
"#;
        let report = check(old, new);
        // 応答の規則 (required → 任意) と要求の規則 (新しい required) の両方で拾う
        assert_breaking(&report, "\"a\"");
        assert_breaking(&report, "\"b\"");
        assert!(
            report.warnings.iter().any(|w| w.contains("\"Loose\"")),
            "向きが分からないことを警告すべき: {report:?}"
        );
    }

    #[test]
    fn unreached_message_is_skipped_with_skip_option() {
        let old = r#"
message "Loose" {
    field "a" type="string" required=#true
}
"#;
        let new = r#"
message "Loose" {
    field "a" type="string"
}
"#;
        let opts = Options {
            unknown_direction: UnknownDirection::Skip,
            ..Options::default()
        };
        let report = check_with(old, new, opts);
        assert_clean(&report);
        assert!(
            report.warnings.iter().any(|w| w.contains("\"Loose\"")),
            "{report:?}"
        );
    }

    // ---- enum は向きで逆になる ----

    fn enum_schema(values: &str, used_in_request: bool) -> String {
        let (req, ret) = if used_in_request {
            (r#"field "v" type="Visibility""#, "")
        } else {
            ("", r#"field "v" type="Visibility""#)
        };
        format!(
            r#"
enum "Visibility" {{
    values {values}
}}
{}"#,
            channel(req, ret)
        )
    }

    #[test]
    fn request_enum_value_removed_is_breaking() {
        let old = enum_schema(r#""private" "shared""#, true);
        let new = enum_schema(r#""private""#, true);
        assert_breaking(&check(&old, &new), "\"shared\"");
    }

    #[test]
    fn response_enum_value_removed_is_clean() {
        let old = enum_schema(r#""private" "shared""#, false);
        let new = enum_schema(r#""private""#, false);
        assert_clean(&check(&old, &new));
    }

    /// 約束に enum と書くのは client が fallback 無しの enum で読むときだけ
    /// (寛容に読むなら約束を `type="string"` にする)。 なので値の追加は既定で壊れる。
    #[test]
    fn response_enum_value_added_is_breaking_by_default() {
        let old = enum_schema(r#""private""#, false);
        let new = enum_schema(r#""private" "public""#, false);
        assert_breaking(&check(&old, &new), "\"public\"");
    }

    #[test]
    fn response_enum_value_added_only_warns_when_lenient() {
        let old = enum_schema(r#""private""#, false);
        let new = enum_schema(r#""private" "public""#, false);
        let opts = Options {
            lenient_enum_additions: true,
            ..Options::default()
        };
        let report = check_with(&old, &new, opts);
        assert_clean(&report);
        assert!(
            report.warnings.iter().any(|w| w.contains("\"public\"")),
            "{report:?}"
        );
    }

    #[test]
    fn request_enum_value_added_is_clean() {
        let old = enum_schema(r#""private""#, true);
        let new = enum_schema(r#""private" "public""#, true);
        let report = check(&old, &new);
        assert_clean(&report);
        assert!(report.warnings.is_empty(), "{report:?}");
    }

    // ---- 入力の誤り ----

    #[test]
    fn parse_error_is_reported() {
        assert!(diff("protocol {", "", &Options::default()).is_err());
    }
}
