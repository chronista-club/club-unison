# unison-mcp-probe

Unison Protocol サーバを Claude Code から対話的につつくための **MCP probe**。

プロダクション bridge ではなく、**開発中に Unison サーバの動作確認をするためのツール**です。

## インストール (プロジェクト内で利用)

`.mcp.json` に追加:

```json
{
  "mcpServers": {
    "unison-probe": {
      "type": "stdio",
      "command": "cargo",
      "args": ["run", "-p", "unison-mcp-probe", "--release"]
    }
  }
}
```

## 提供 Tool

| Tool | 引数 | 役割 |
|------|------|------|
| `unison_ping` | `endpoint`, `trust` | Unison サーバへの疎通確認 |
| `unison_call` | `endpoint`, `channel_name`, `method`, `payload`, `trust` | channel を open して method を request 送信、 response を取得 |

## 設計判断

当初は「KDL → MCP tool 自動変換 bridge」として起票 (USN-2) していたが、
**動作確認・トリガー用途が最優先**と判明し、probe にスコープ縮小。

| 側面 | 選択 | 理由 |
|------|------|------|
| mapping | request=tool (手書き) | 自動変換は不要 |
| persistence | stateless (毎回 open/close) | MCP server プロセスは stateless 運用で十分 |
| 認証 | 不要 | local stdio 前提 |
| 双方向 | 片方向 | stateless から自動決定 |
| ビルド | 手書き | codegen 不要 |

## Future (必要になれば別 issue 起票)

- KDL schema からの自動 tool 生成
- `event` → MCP notification のマッピング
- 永続 channel / connection pool
- 認証・マルチテナント

## 関連

- Linear: [USN-2](https://linear.app/chronista/issue/USN-2) — スコープ詳細
- 親: [USN-1](https://linear.app/chronista/issue/USN-1) — 発想ストック
- MCP SDK: [rmcp (公式)](https://github.com/modelcontextprotocol/rust-sdk)
