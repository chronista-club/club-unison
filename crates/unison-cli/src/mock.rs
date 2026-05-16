//! `unison mock --schema <file.kdl>` — KDL schema から stub server を起動。
//!
//! schema 内の各 `channel` を `ProtocolServer::register_channel` で登録し、
//! client から来た request に対して schema の `returns` 型から組み立てた
//! stub payload を返す。実バックエンド無しで client 開発を進めるための偽サーバ。
//!
//! stub 応答は field 型から決定的に生成する (string→"", int→0, bool→false,
//! json/object→{}, float→0.0)。`returns` を持たない request には空 object を返す。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use unison::network::quic::UnisonStream;
use unison::network::{MessageType, UnisonChannel};
use unison::parser::{ChannelBackend, Field, FieldType, SchemaParser};
use unison::{ProtocolServer, UnisonProtocol};

#[derive(Args)]
pub struct MockArgs {
    /// mock の元になる KDL channel schema
    #[arg(long)]
    pub schema: PathBuf,

    /// bind アドレス
    #[arg(long, default_value = "[::1]:7878")]
    pub addr: String,
}

/// 1 request の stub 仕様: method 名 → 返す payload。
type StubTable = HashMap<String, serde_json::Value>;

pub async fn run(args: MockArgs) -> Result<()> {
    let src = std::fs::read_to_string(&args.schema)
        .with_context(|| format!("failed to read {}", args.schema.display()))?;

    // parse + validate (= schema-lint と同じ入口)
    let parser = SchemaParser::new();
    let schema = parser
        .parse(&src)
        .context("schema parse failed — run `unison schema-lint` for detail")?;
    // load_schema 経由 sanity check
    UnisonProtocol::new()
        .load_schema(&src)
        .context("schema rejected by UnisonProtocol::load_schema")?;

    let protocol = schema
        .protocol
        .as_ref()
        .context("schema has no `protocol` block")?;

    let server = ProtocolServer::with_identity(
        &format!("unison-mock:{}", protocol.name),
        env!("CARGO_PKG_VERSION"),
        "mock",
    );

    let mut registered = 0usize;
    for channel in &protocol.channels {
        if channel.backend() == ChannelBackend::Datagram {
            // datagram channel は request 不可、stub 応答も無いので skip (= warn)
            eprintln!(
                "skip datagram channel '{}' — mock only stubs request/response channels",
                channel.name
            );
            continue;
        }

        // この channel の各 request の stub payload を事前計算
        let mut stubs: StubTable = HashMap::new();
        for req in &channel.requests {
            let payload = req
                .returns
                .as_ref()
                .map(|r| stub_object(&r.fields))
                .unwrap_or_else(|| serde_json::json!({}));
            stubs.insert(req.name.clone(), payload);
        }
        let stubs = Arc::new(stubs);
        let chan_name = channel.name.clone();

        server
            .register_channel(&channel.name, move |_ctx, stream| {
                let stubs = Arc::clone(&stubs);
                let chan_name = chan_name.clone();
                async move { handle_channel(chan_name, stubs, stream).await }
            })
            .await;
        registered += 1;
    }

    println!(
        "unison mock — protocol \"{}\" v{}",
        protocol.name, protocol.version
    );
    println!("  schema:  {}", args.schema.display());
    println!("  channels: {registered} stubbed");
    println!("  listening on {} — Ctrl-C to stop", args.addr);

    server
        .listen(&args.addr)
        .await
        .context("server listen failed")?;
    Ok(())
}

/// 1 つの channel stream を捌く: request を recv し stub response を返す loop。
async fn handle_channel(
    chan_name: String,
    stubs: Arc<StubTable>,
    stream: UnisonStream,
) -> Result<(), unison::network::NetworkError> {
    let channel: UnisonChannel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                let reply = stubs
                    .get(&msg.method)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                tracing::info!(channel = %chan_name, method = %msg.method, "mock stub reply");
                channel
                    .send_response(msg.id, &msg.method, &reply)
                    .await?;
            }
            Ok(msg) => {
                // Event 等 — mock は受け流すだけ
                tracing::debug!(channel = %chan_name, method = %msg.method, "mock ignored non-request");
            }
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

/// field 群から stub の JSON object を組み立てる。
fn stub_object(fields: &[Field]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for f in fields {
        map.insert(f.name.clone(), stub_value(&f.field_type()));
    }
    serde_json::Value::Object(map)
}

/// field 型から決定的な stub 値を生成する。
fn stub_value(ty: &FieldType) -> serde_json::Value {
    use serde_json::{Value, json};
    match ty {
        FieldType::String | FieldType::Custom(_) | FieldType::Enum(_) => json!(""),
        FieldType::Int => json!(0),
        FieldType::Float => json!(0.0),
        FieldType::Bool => json!(false),
        FieldType::Json | FieldType::Object => json!({}),
        FieldType::Array(_) => Value::Array(vec![]),
        FieldType::Map(_, _) => json!({}),
    }
}
