# Unison Protocol — AI開発ガイド

## 基本方針

- **丁寧さ > 速度**: 急がず、質の高いコード・ドキュメントを残すことを優先する
- **Legacy は残さない**: deprecated / 後方互換のためだけの実装は不要。不要なコードは削除する
- **Minimum を保つ**: 必要最小限の状態を維持する。過剰な抽象化・冗長なコードを避ける

## アーキテクチャ

### Unified Channel

全通信はチャネル経由で行う。RPC は廃止済み。

- `UnisonChannel`: 統合チャネル型（request/response + event push）
- `register_channel()`: サーバー側チャネルハンドラー登録
- `open_channel()`: クライアント側チャネル開設（`UnisonChannel` を返す）

### KDL スキーマ

チャネル定義は `request` / `returns` / `event` 構文を使用:

```kdl
channel "name" from="client" lifetime="persistent" {
    request "Name" {
        field "key" type="string"
        returns "Response" {
            field "data" type="json"
        }
    }
    event "EventName" {
        field "code" type="string"
    }
}
```

旧 `service` / `method` / `send` / `recv` 構文は非推奨。

## テスト

```bash
# 標準テスト実行 (lib unit + Small の integration test)
RUSTFLAGS="-C symbol-mangling-version=v0" cargo test --workspace

# 実 QUIC を使う Medium test (= #[ignore] 付き)
RUSTFLAGS="-C symbol-mangling-version=v0" cargo test --workspace -- --ignored

# clippy (lib / bins / tests / benches / examples を同じ厳しさで)
cargo clippy --all-targets --workspace -- -D warnings
```

### テストファイルの命名 = テストの層

`crates/*/tests/` のファイル名は `test_<layer>_<topic>.rs`。 **層は名前で分かる**。

| 層 | 意味 | `#[ignore]` | ファイル名 |
|----|------|------------|-----------|
| `small` | 実 I/O なし。 parser / wire / in-memory の handler | なし (常時実行) | `test_small_*.rs` |
| `medium` | 実 QUIC connection を localhost で張る | あり | `test_medium_*.rs` |

Medium は `#[ignore = "Medium: 実 QUIC runtime が要る"]` で理由を揃える。 例外は
`test_medium_accept_resilience.rs` と `test_medium_alpn_enforcement.rs` で、
過去に実害を出したバグの回帰テストなので `#[ignore]` を付けず常時走らせる
(= handshake だけで完結し数百 ms で終わるため)。

Large (= 別プロセス / 複数言語) は Rust の `tests/` ではなく CI の
`Cross-Language E2E` job と `clients/*` 側が担う。

## ドキュメント構造

| ディレクトリ | 用途 |
|-------------|------|
| `spec/` | 仕様（What & Why） |
| `design/` | 設計（How） |
| `guides/` | 使い方ガイド |

Living Documentation 原則: ドキュメントとコードは常に同期させる。
