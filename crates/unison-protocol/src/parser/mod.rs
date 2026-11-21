use club_kdl::{Document, Node};
use thiserror::Error;

pub mod check;
pub mod schema;

pub use check::{TypeIndex, Violation};
pub use schema::*;

/// KDL スキーマの parse / 検証エラー。
#[derive(Error, Debug)]
pub enum ParseError {
    /// KDL 文法として読めない、 あるいは [`ParsedSchema`] の形に合わない。
    #[error("KDL parsing error: {0}")]
    Kdl(String),
    /// 文法は通ったが、 スキーマの意味的制約に反する
    /// (= datagram channel の `channel_id` 欠落、 `readonly` と `destructive` の同時宣言 等)。
    #[error("Schema validation error: {0}")]
    Validation(String),
}

/// Main schema parser for KDL protocol definitions
pub struct SchemaParser;

impl SchemaParser {
    pub fn new() -> Self {
        Self
    }

    /// Parse a KDL schema string into a ParsedSchema
    ///
    /// 自分が書いた schema 用 (= server が配るもの、 lint するもの)。 その位置に書けない
    /// 名前のノード (= 旧 `service` / `send` / `recv` 構文、 タイプミス) は
    /// [`ParseError::Kdl`] で弾く。 パース後に [`Channel::validate`] を全 channel に
    /// 対して呼び、 datagram channel の `channel_id` 必須性等の semantic constraint を
    /// 検証する。 相手から受け取った schema には [`Self::parse_lenient`] を使う。
    pub fn parse(&self, input: &str) -> Result<ParsedSchema, ParseError> {
        Self::parse_with(input, ForeignNodes::Reject)
    }

    /// 他の方言の node を top level に持つ「約束」を読む (= `schema-check` /
    /// `schema-diff` 用。 例: Creo の REST の約束にある `route`)。
    ///
    /// top level の知らない node は子孫ごと読み飛ばす。 Unison の node (`message` /
    /// `enum` / `protocol` 等) の中は [`Self::parse`] と同じく厳密に見る。
    pub fn parse_contract(&self, input: &str) -> Result<ParsedSchema, ParseError> {
        Self::parse_with(input, ForeignNodes::SkipAtTopLevel)
    }

    /// 相手から受け取った schema を読む (= discovery で server から届いたもの)。
    ///
    /// この版が知らない node は、 どの位置にあっても読み飛ばす。 厳密に見るのは
    /// schema を書く側の役目で、 受け取る側が弾くと、 新しい版の語彙を使う server に
    /// 古い client が繋げなくなる (= 知っている channel / request まで使えなくなる)。
    /// KDL として壊れているもの、 [`Channel::validate`] に反するものは弾く。
    pub fn parse_lenient(&self, input: &str) -> Result<ParsedSchema, ParseError> {
        Self::parse_with(input, ForeignNodes::Skip)
    }

    fn parse_with(input: &str, foreign: ForeignNodes) -> Result<ParsedSchema, ParseError> {
        let doc = Document::parse(input).map_err(|e| ParseError::Kdl(e.to_string()))?;
        if foreign != ForeignNodes::Skip {
            for node in doc.nodes() {
                if foreign == ForeignNodes::SkipAtTopLevel
                    && !TOP_LEVEL_NODES.contains(&node.name())
                {
                    continue;
                }
                reject_unknown_nodes(std::slice::from_ref(node), TOP_LEVEL_NODES, "")?;
            }
        }
        let schema: ParsedSchema =
            club_kdl::from_doc(&doc).map_err(|e| ParseError::Kdl(e.to_string()))?;

        // Channel semantic validation (v0.10.0 で導入: datagram channel の channel_id 必須性等)
        if let Some(ref protocol) = schema.protocol {
            for channel in &protocol.channels {
                channel.validate().map_err(ParseError::Validation)?;
            }
        }

        Ok(schema)
    }
}

/// Unison の語彙に無い node の扱い。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ForeignNodes {
    /// どの位置でも弾く (= 自分が書いた schema)
    Reject,
    /// top level だけ読み飛ばす (= 他の方言の node を持つ約束)
    SkipAtTopLevel,
    /// どの位置でも読み飛ばす (= 相手から受け取った schema)
    Skip,
}

/// ファイルの一番上に書いてよいノード。
const TOP_LEVEL_NODES: &[&str] = &["protocol", "import", "message", "enum", "typedef"];

/// 各ノードの中に書いてよい子ノード。 [`schema`] の derive (`#[kdl(child)]` /
/// `#[kdl(children, name = ..)]`) と揃える。 club-kdl の derive は名前の一致する
/// 子ノードだけを拾って残りを黙って捨てるので、 ここに無い名前は
/// [`reject_unknown_nodes`] が弾く。 表に無いノード (`field`、 値だけを持つ
/// `namespace` / `description` / `values` 等) は子を持たない。
fn allowed_children(node: &str) -> &'static [&'static str] {
    match node {
        "protocol" => &["namespace", "description", "message", "enum", "channel"],
        "channel" => &["request", "event"],
        "request" => &["field", "returns"],
        "returns" | "event" => &["field"],
        "message" => &["description", "field"],
        "enum" => &["values"],
        "typedef" => &[
            "base_type",
            "rust_type",
            "typescript_type",
            "format",
            "pattern",
        ],
        _ => &[],
    }
}

/// `nodes` が `allowed` の名前だけで書かれているかを、 子孫まで辿って確かめる。
///
/// AST に行番号が無いので、 場所は `protocol "x" > channel "users"` のような
/// ノードの経路で示す。
fn reject_unknown_nodes(
    nodes: &[Node<'_>],
    allowed: &[&str],
    path: &str,
) -> Result<(), ParseError> {
    for node in nodes {
        let name = node.name();
        if !allowed.contains(&name) {
            let place = if path.is_empty() {
                "at the top level".to_string()
            } else {
                format!("in {path}")
            };
            let hint = if allowed.is_empty() {
                "no child nodes allowed".to_string()
            } else {
                format!("allowed: {}", allowed.join(", "))
            };
            return Err(ParseError::Kdl(format!(
                "unknown node \"{name}\" {place} ({hint})"
            )));
        }
        let label = match node.arg(0).and_then(|v| v.as_string()) {
            Some(arg) => format!("{name} \"{arg}\""),
            None => name.to_string(),
        };
        let child_path = if path.is_empty() {
            label
        } else {
            format!("{path} > {label}")
        };
        reject_unknown_nodes(node.children(), allowed_children(name), &child_path)?;
    }
    Ok(())
}

impl Default for SchemaParser {
    fn default() -> Self {
        Self::new()
    }
}
