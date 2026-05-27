# unison-mcp

MCP (Model Context Protocol) bridge for the Unison Protocol — discovers a server's protocol schema at runtime and exposes tools to AI agents (e.g. Claude Code).

> **後継位置づけ**: `unison-mcp-probe` の superset。 probe の `unison_channel_list` TODO を `unison_discover` が埋める。 probe は Unison Hailing α P3c で deletion 予定。

## Status

**Hailing α Epic — P3a scaffold (2026-05-28 in progress)**

| feature | status |
|---|---|
| `unison_ping` (escape hatch) | ✅ ported from probe |
| `unison_call` (escape hatch) | ✅ ported from probe |
| `unison_discover` (NEW) | ✅ wraps `DynamicProtocol::fetch` |
| Synthesized typed tools (`unison.<channel>.<method>`) | ⏳ P3b |
| MCP E2E demo with Claude Code | ⏳ P3c |

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

### `unison_discover` (NEW)

Fetch the server's protocol KDL via the `unison.discovery` channel, return a summary of channels / requests / events.

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

- `crates/unison-protocol` — core protocol (= `DynamicProtocol`, `SchemaRegistry`, `ProtocolCache`)
- `crates/unison-mcp-probe` — legacy probe (= scheduled for deletion in P3c)
- `spec/04-discovery/SPEC.md` — discovery channel specification
