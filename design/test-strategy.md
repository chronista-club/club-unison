# テスト戦略

**最終更新**: 2026-11-04

テストの層（Small / Medium / Large）の定義、ファイル命名（`test_<layer>_<topic>.rs`）、
`#[ignore]` の理由文字列と例外 2 本、実行コマンドは repo root の [CLAUDE.md](../CLAUDE.md)
「テスト」節が正。本書はここで繰り返さず、その背景と「どこに何があるか」を書く。

## 1. テスト哲学

### t-wada 3x3 マトリクス

テストを **サイズ**（実行コスト）と **スコープ**（検証範囲）の 2 軸で分類する。

| | Unit | Integration | E2E |
|---|---|---|---|
| **Small** | 単一関数・構造体の検証。外部依存なし、インメモリのみ | 複数モジュールを結合。Mock/Fake で外部依存を置換 | — |
| **Medium** | — | ネットワーク・ファイルI/O を含む結合テスト | 複数プロセス間の結合 |
| **Large** | — | — | 実環境に近い構成での検証 |

Unison のファイル名が表すのは **サイズ** の軸（`small` = 実 I/O なし、`medium` = 実 QUIC を
localhost で張る）。スコープはファイル名に出さない。

### 核心原則: 「サイズを上げずにスコープを広げる」

- 部品は lib unit test（`src/**` の `#[cfg(test)] mod tests`）で固める
- 複数モジュールの結合も、できるだけ実 I/O なし（= Small）で確かめる。wire は bytes の往復、
  handler 登録と identity 構築は in-memory、接続ディスパッチは `MockConn`（§2.1）で回す
- 実 QUIC を張る Medium は、transport を通さないと見えない振る舞い（handshake / ALPN /
  証明書 / datagram / 接続 lifecycle / dial race など）に絞る

### Rust の型システム = 静的解析層

Testing Trophy の最下層（Static Analysis）は Rust コンパイラが自然にカバーする:

- 型チェック・借用チェック → Null 参照・データ競合の排除
- `clippy` → コードスメル・パターン違反の検出（lib / bins / tests / benches / examples を同じ厳しさで見る）
- 型による不正状態の排除（`enum` + exhaustive match）

この強固な静的解析層があるからこそ、テストは **ロジックの正しさ** と **モジュール間の結合** に集中できる。

## 2. どこに何があるか

### 2.1 crate 内（lib unit test）

- `crates/unison-protocol/src/` は `codec/` / `network/` / `packet/` / `parser/` の 4 つ
  （構成は [architecture.md](./architecture.md) §3）。各 module の `#[cfg(test)] mod tests` に置く
- `network::test_support::MockConn`（`#[cfg(test)]`、crate 内専用）は実 QUIC なしで
  `UnisonConn` を差し込む fake。`dispatch::handle_connection` などを Small のまま回せる
- `network::dial` の engine は `#[tokio::test(start_paused = true)]` の仮想時間で決定論的に
  検証する（[happy-eyeballs-dial.md](./happy-eyeballs-dial.md) §7）
- `unison-cli` / `unison-mcp` も各 `src/` に unit test を持つ

### 2.2 `crates/unison-protocol/tests/`

**Small**（常時実行）:

| ファイル | 対象 |
|---|---|
| `test_small_cert_trust_config.rs` | `CertSource` / `TrustAnchors` から rustls の config を組めるか |
| `test_small_contract_check.rs` | `parser::check`（[schema-check.md](./schema-check.md)） |
| `test_small_creo_sync_schema.rs` | fixture `creo_sync.kdl` の channel 型 |
| `test_small_error_paths.rs` | packet / `ProtocolMessage` の異常入力 |
| `test_small_identity_flow.rs` | `ServerIdentity` ↔ `ProtocolMessage`、`ConnectionContext` の identity |
| `test_small_kdl_schema.rs` | KDL の parse（channel / message / enum / backend） |
| `test_small_protocol_message.rs` | `ProtocolMessage` の `into_frame` → bytes → `from_frame` 往復 |
| `test_small_server_handlers.rs` | `ProtocolServer` の channel 登録、`build_identity`、connection event の購読 |
| `test_small_wire_byte_compat.rs` | wire の golden（`fixtures/wire/*.hex`）との byte 比較（[packet.md](./packet.md) §6） |

**Medium**（`#[ignore]`、`-- --ignored` で実行）:

| ファイル | 対象 |
|---|---|
| `test_medium_accept_resilience.rs` | accept loop の回帰。**`#[ignore]` なしで常時実行** |
| `test_medium_alpn_enforcement.rs` | ALPN の回帰（[quic-runtime.md](./quic-runtime.md) §1）。**`#[ignore]` なしで常時実行** |
| `test_medium_auth_channel.rs` | `unison.auth`（[connection-auth.md](./connection-auth.md)） |
| `test_medium_client_events.rs` | `ProtocolClient` の Connected / Disconnected event |
| `test_medium_connect_race.rs` | `connect_race`（[happy-eyeballs-dial.md](./happy-eyeballs-dial.md)） |
| `test_medium_datagram_echo.rs` | datagram channel の echo / demux / broadcast（[datagram-channel.md](./datagram-channel.md)） |
| `test_medium_discovery_channel.rs` | `unison.discovery` |
| `test_medium_dynamic_schema.rs` | `DynamicProtocol` + `SchemaRegistry` |
| `test_medium_ip_dualstack.rs` | IPv4 / IPv6 の疎通 |
| `test_medium_mesh_trust.rs` | mesh CA trust の SAN 名前検証 |
| `test_medium_quic_lifecycle.rs` | bind / connect / identity / request / shutdown |
| `test_medium_quic_protocol_flow.rs` | schema → server → client → channel の一気通貫 |
| `test_medium_server_initiated_stream.rs` | `open_server_stream` / `register_server_channel`（[server-initiated-stream.md](./server-initiated-stream.md)） |
| `test_medium_spawn_cert.rs` | 高レベルの spawn 経路が cert を honor するか |

`test_medium_server_initiated_stream.rs` のうち `test_open_server_stream_on_client_ctx_is_misuse_error`
は ctx 単体の検査で QUIC を張らないため、`#[ignore]` が付いていない。

### 2.3 `crates/unison-mcp/tests/`

| ファイル | 対象 |
|---|---|
| `test_medium_synthesis.rs` | discovery server → unison-mcp の tool 合成 / invoke / live refresh（[kdl-to-json-schema.md](./kdl-to-json-schema.md) §6） |

### 2.4 共通ヘルパーと fixture

- `tests/common/mod.rs`（`crates/unison-protocol/tests/`）:
  - `make_message` / `make_request` / `make_event` — JSON payload で `ProtocolMessage` を組む
  - `make_identity` — channel 名の一覧から `ServerIdentity` を組む
  - Small の一部（`test_small_error_paths.rs` / `test_small_identity_flow.rs` /
    `test_small_protocol_message.rs` / `test_small_server_handlers.rs`）が `mod common;` で使う
- `tests/fixtures/`: `wire/*.hex`（wire の golden）、`creo_sync.kdl`、`contracts/`（schema-check 用の約束の抜粋）

### 2.5 Medium test の書き方

- server は `Arc::new(server).listener("[::1]:0").spawn().await?` で起動し、`handle.local_addr()`
  で実ポートを得る。port 0 なので並列に走らせても衝突しない
- client は多くのテストで `ProtocolClient::insecure_localhost()` を使う（検証なし、loopback 限定）
- 片付けは `ServerHandle::shutdown()`
- 各テストに `#[ignore = "Medium: 実 QUIC runtime が要る"]` を付ける（例外は CLAUDE.md）

### 2.6 Large

Rust の `tests/` には置かない。

- CI の `Cross-Language E2E` job が `webtransport_echo_server` example を build し、TS の
  `npm test`（`clients/typescript/tests/integration/webtransport_e2e.test.ts` ほか）が実
  WebTransport で Rust server と往復する
- `clients/*` 側のテスト: TS（`clients/typescript/tests/`）、Swift（`swift test`。live e2e は
  `UNISON_LIVE` で gate）、Ruby（`rake test`。`test/e2e/` は `unison` バイナリが要るので既定では走らない）

## 3. CI での走り方

`.github/workflows/ci.yml` の job 一覧は [architecture.md](./architecture.md) §2。テストに関わる
ところだけ書く:

| job | 実行するもの |
|---|---|
| Test（ubuntu + macos） | `cargo test --workspace --verbose`（lib unit + Small + 例外 2 本）→ `cargo test --workspace --verbose -- --ignored`（Medium）。macOS は `RUSTFLAGS=-C symbol-mangling-version=v0` |
| Clippy | `cargo clippy --all-targets --workspace -- -D warnings` |
| Cross-Language E2E | §2.6 |
| Ruby Client / Swift Client / TS Typecheck | 各 client の build と test / 型検査 |

## 4. 目標

| 指標 | 目標値 | 根拠 |
|------|--------|------|
| サイズ比率 | Small 80% / Medium 15% / Large 5% | Google Testing Blog 推奨比率 |
| `cargo test` 実行時間 | < 30 秒（Small のみ） | 開発中のフィードバック速度 |
| CI 全体 | < 10 分 | PR マージまでの待ち時間 |
| Flaky 率 | 0%（1% 超でエンジニアの信頼が崩壊） | t-wada「テストの信頼性」原則 |

## 5. 偽陽性・偽陰性対策

### 偽陽性（False Positive）: テストが壊れたが実装は正しい

- **内部実装に依存しないテスト設計**: private フィールドの値ではなく、公開 API の振る舞いを検証する
- **文字列マッチングの回避**: error message などの `contains("exact string")` に頼りすぎない → 構造的な検証を併用
- **テスト同士の独立性**: 共有状態を持たない。各テストは独立して実行可能

### 偽陰性（False Negative）: テストが通るが実装にバグがある

- **エラーパステスト**: 正常系だけでなく、異常入力・境界値を必ずテスト
- **期待値をテスト自身が書き換えない**: golden は commit 済みのものと比較するだけで、再生成は
  明示したとき（`UPDATE_WIRE_FIXTURES=1`）に限る。毎回上書きすると回帰を見逃す
- **proptest 検討**: パケットのシリアライゼーション等、入力空間が広い箇所にはプロパティベーステストが有効
- **カバレッジ計測**: `cargo-tarpaulin` で定期的に未カバー行を可視化
