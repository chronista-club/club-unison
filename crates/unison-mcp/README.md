# unison-mcp

MCP (Model Context Protocol) bridge for the Unison Protocol — discovers a server's protocol schema at runtime and exposes tools to AI agents (e.g. Claude Code).

## Status

**rmcp 3.x 世代** (= 1.9.0 で rmcp 3 へ、 MCP spec 2025-06-18+ の機能を還元)

| feature | status |
|---|---|
| `unison_ping` (= static escape hatch) | ✅ |
| `unison_call` (= static escape hatch、 generic、 schema 検証なし) | ✅ |
| `unison_discover` (= unison.discovery 経由で server KDL を fetch + summary) | ✅ |
| Synthesized typed tools (= `unison_<channel>_<method>`、 起動時 discovery 成功時) | ✅ |
| Synthesized `output_schema` (= KDL `returns` block から合成、 入出力とも typed) | ✅ |
| `structuredContent` results (= 全 tool、 text は互換 mirror) | ✅ |
| Tool annotations + title (= read-only/idempotent hint、 `channel.method` 表示名) | ✅ |
| Live re-discovery (= `unison_discover` が default endpoint の tool set を refresh、 hash 変化で `tools/list_changed`) | ✅ |
| Endpoint elicitation (= endpoint 未設定時、 対応 client へその場で質問。 30 秒で error に落ちる) | ✅ |
| 断られた request / 形の違う返事を、 理由と中身ごと agent に返す (`isError`) | ✅ |
| MCP E2E demo (= Claude Code から実機 round-trip) | ✅ (DEMO.md 参照) |

## Install

```bash
cargo build -p unison-mcp --release
# binary: target/release/unison-mcp
```

## Use with Claude Code

Add to `.mcp.json`:

```json
{
  "mcpServers": {
    "unison": {
      "type": "stdio",
      "command": "/absolute/path/to/target/release/unison-mcp",
      "args": ["--config", "/absolute/path/to/unison.json"],
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

`--config` is optional. Without it, every tool call must include the `endpoint` argument explicitly.

### Example `unison.json`

See [`examples/unison.json`](examples/unison.json):

```json
{
  "endpoint": "quic://[::1]:7878",
  "trust": "skip"
}
```

- `endpoint` — default Unison server URL (= overridable per tool call)
- `trust` — `"skip"` (dev、 self-signed) or `"system"` (= OS / webpki-roots)

## Tools

成功した結果は `structuredContent`（= 機械可読 JSON、 text content は互換 mirror）で返る。
synthesized tools は KDL `returns` block から `output_schema` を宣言するので、 client 側で
結果の型検証が可能。

**server の返事は、 agent から隠さない**:

- server の handler が request を断った（`UnisonChannel::send_error`）、 timeout、 接続断は、
  tool の結果（`isError: true`）として返る。 text に理由が入る。
- 返事が `returns` の形と違う場合（例: 成功の frame に `{"error": "..."}` を詰めて返す
  server）も、 `isError: true` の結果として、 どこが違うかと**返事の中身そのもの**を text で返す。
  `structuredContent` には載せない（載せると MCP client の output schema 検証が返事ごと
  落とし、 中身が agent に届かなくなる）。
- `output_schema` は、 `required` でない field に null を許す（Rust の `Option` は null に
  なるため）。
- 入力が schema に合わない場合だけは、 server に送る前に `invalid_request` で返す（どの
  field が違うかが文面に入る）。

endpoint が config にも tool arg にも無い場合、 elicitation 対応 client にはその場で接続先を
質問する。 30 秒以内に答えが無ければ（= 答える人がいない session）、 非対応 client と同じく
「`endpoint` を渡すか `--config` を」という error に落ちる。

### `unison_ping`

Verify connectivity to an endpoint.

| Arg | Type | Required |
|---|---|---|
| `endpoint` | string | only if no default in config |
| `trust` | `"skip"` / `"system"` | no (default: config or `"skip"`) |

### `unison_call` (escape hatch)

Send any request to a channel. No schema validation — useful for debugging or unknown channels.

| Arg | Type | Required |
|---|---|---|
| `endpoint` | string | only if no default in config |
| `channel_name` | string | yes |
| `method` | string | yes (= KDL `request "Name"` の Name) |
| `payload` | JSON | yes |
| `trust` | `"skip"` / `"system"` | no |

### `unison_discover`

Fetch the server's protocol KDL via the `unison.discovery` channel, return a summary of channels / requests / events.

**Refresh semantics**: default endpoint (= config の endpoint) に対する discover は、
成功時に bridge の discovery を置き換えて synthesized tool set を更新する
(= server の schema 進化に MCP session 中に追従)。 protocol hash が変わった場合は
`notifications/tools/list_changed` を client に送る。 明示的に別 endpoint を指定した
discover は one-off 照会で、 state を変えない (= response の `refreshed` field で判別可)。

| Arg | Type | Required |
|---|---|---|
| `endpoint` | string | only if no default in config |
| `trust` | `"skip"` / `"system"` | no |

**Response shape**:

```json
{
  "endpoint": "[::1]:7878",
  "protocol_name": "my-protocol",
  "version": "1.0.0",
  "namespace": "my.namespace",
  "hash": "abc...64-char-hex",
  "codecs": ["json"],
  "channels": [
    {
      "name": "chat",
      "from": "client",
      "lifetime": "persistent",
      "backend": "stream",
      "requests": ["Send", "Subscribe"],
      "events": ["Received"]
    }
  ]
}
```

## Architecture

```
Claude Code (AI agent)
        │ MCP (stdio)
        ▼
   unison-mcp
        │ rmcp + Unison ProtocolClient
        ▼
   Unison Server
        │ register_channel("unison.discovery", ...)
        ▼
   ProtocolCache (= KDL + SHA-256 hex)
```

Internally:

- `src/main.rs` — clap arg parsing + stdio MCP entry
- `src/config.rs` — `BridgeConfig` (optional default endpoint + trust)
- `src/bridge.rs` — `UnisonBridge` (= shared state)
- `src/tools.rs` — 3 tool implementations (= `unison_ping`, `unison_call`, `unison_discover`)

## Related

- [`crates/unison-protocol`](../unison-protocol) — core protocol (= `DynamicProtocol`, `SchemaRegistry`, `ProtocolCache`)
- [`spec/04-discovery/SPEC.md`](../../spec/04-discovery/SPEC.md) — discovery channel specification
- [`DEMO.md`](DEMO.md) — Claude Code を driver にした E2E demo 手順
