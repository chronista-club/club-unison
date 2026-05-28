# Hailing α MCP demo

Runtime protocol discovery + synthesized typed MCP tools — Claude Code から任意の Unison server を 「初見で typed call」 できる demo。 Unison Hailing α Epic の close 条件。

## Demo の到達点

```
$ claude --mcp-config <path>/unison.json
> "hailing-demo server で 3 + 4 を計算して、 結果を 'echo' channel に echo back して"

[Claude が tools/list を見る]
   - unison_ping / unison_call / unison_discover  (static escape hatches)
   - unison_unison_discovery_GetProtocol           (self-describing meta)
   - unison_greet_Hello                            (Greet by name)
   - unison_math_Add                               (Integer addition)
   - unison_echo_Echo                              (JSON echo)

[Claude が unison_math_Add を invoke]
   args: {"a": 3, "b": 4}
   ↓ DynamicChannel.request("Add", {...}) → SchemaRegistry.validate_request → OK
   ↓ Unison server へ送信
   ↓ result: {"result": 7}

[Claude が unison_echo_Echo を invoke]
   args: {"payload": {"result": 7}}
   ↓ ...
   ↓ result: {"echoed": {"result": 7}}

[Claude が user に答える]
   "3 + 4 = 7 で、 echo channel に echo back しました"

✅ Hailing α demo 完了
```

## 必要なもの

- Rust toolchain (mise が設定済の場合は自動)
- Claude Code CLI (`claude` command が PATH に)
- このリポジトリの clone (= `~/repos/club-unison`)

## 手順

### 1. Build

```bash
# unison-mcp binary を release build
cargo build --release -p unison-mcp

# binary は target/release/unison-mcp に
ls -la target/release/unison-mcp
```

### 2. Demo server を起動 (= 別 terminal で常駐)

```bash
cargo run --release -p club-unison --example hailing_demo_server
```

期待 output:
```
✓ unison.discovery channel enabled (= P1)
✓ greet channel registered (Hello)
✓ math channel registered (Add)
✓ echo channel registered (Echo)
================================================================
  Hailing α demo server — protocol "hailing-demo" v0.1.0
  Listening on [::1]:7878
  ...
  Press Ctrl-C to stop
================================================================
```

このまま放置 (= 接続待ち状態)。

### 3. Claude Code に unison-mcp を登録

`~/.claude/mcp.json` (= user-level) または project の `.mcp.json` に以下を追加:

```json
{
  "mcpServers": {
    "unison": {
      "type": "stdio",
      "command": "/absolute/path/to/club-unison/target/release/unison-mcp",
      "args": [
        "--config",
        "/absolute/path/to/club-unison/crates/unison-mcp/examples/unison.json"
      ],
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

`unison.json` の中身 (= [`crates/unison-mcp/examples/unison.json`](examples/unison.json)):

```json
{
  "endpoint": "quic://[::1]:7878",
  "trust": "skip"
}
```

### 4. Claude Code を起動して demo prompt を投げる

別 terminal で:

```bash
cd /any/directory
claude
```

prompt 例:
- 「unison の tool 一覧を見せて」 → tools/list が走り、 6 tools (= 3 static + 3 synthesized) が見える
- 「unison_math_Add で 3 と 4 を足して」 → DynamicChannel 経由で `{a:3, b:4}` → server → `{result: 7}`
- 「unison_greet_Hello で alice に挨拶して」 → `Hello, alice! 👋`
- 「unison_echo_Echo で {nested: [1,2,3]} を echo して」 → そのまま echoed として返ってくる

### 5. Validation の fail-fast を確認 (= optional)

- 「unison_math_Add で {a: "three"} (= int に string)」 → MCP invalid_request、 server には到達せず
- 「unison_greet_Hello で {} (= name 欠落)」 → MCP invalid_request `name field is required`

これが Hailing α P2-Rust の `SchemaRegistry::validate_request` の fail-fast 効果。

## 録画 (= optional、 demo evidence)

- asciinema: `asciinema rec hailing-demo.cast`
- macOS QuickTime: screen recording
- 録画ファイルは `crates/unison-mcp/demo-recordings/` (= gitignore) に置き、 Epic close memory に reference

## Troubleshoot

- **server 起動時 「Address already in use」**: `lsof -i :7878` で別 process を kill
- **discovery timeout 3s で fail**: demo server が立ち上がってない、 wrong endpoint
- **tools/list が 3 (static only)**: bridge の discovery が fail、 server log + bridge log (`RUST_LOG=debug`) で確認
- **invoke で `validation: ... field missing`**: KDL schema の required field を payload に含めてない (= fail-fast working as intended)

## アーキテクチャ (= 確認用)

```
Claude Code (= AI agent)
        │ stdio MCP (= tools/list, tools/call)
        ▼
   unison-mcp bridge (= this crate)
        │  - eager fetch on startup via unison.discovery
        │  - all_tools() = 3 static + N synthesized
        │  - invoke_tool dispatches to handle_synthesized → DynamicChannel
        ▼ QUIC + TLS 1.3
   hailing-demo server (= examples/hailing_demo_server.rs)
        │  - enable_discovery(DEMO_KDL) で P1 handler 登録
        │  - greet / math / echo handler 登録
        │  - protocol KDL は ProtocolCache が memoize (SHA-256 hex)
```

## Related

- Spec: [`spec/04-discovery/SPEC.md`](../../spec/04-discovery/SPEC.md)
- KDL→JSON Schema 対応: [`docs/kdl-to-json-schema.md`](../../docs/kdl-to-json-schema.md)
- Bridge crate README: [`README.md`](README.md)
- Demo server source: [`../unison-protocol/examples/hailing_demo_server.rs`](../unison-protocol/examples/hailing_demo_server.rs)
