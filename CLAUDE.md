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
# 標準テスト実行 (lib unit + integration tests を全部走らせる)
RUSTFLAGS="-C symbol-mangling-version=v0" cargo test --workspace

# clippy
cargo clippy --lib --workspace -- -D warnings
```

## ドキュメント構造

| ディレクトリ | 用途 |
|-------------|------|
| `spec/` | 仕様（What & Why） |
| `design/` | 設計（How） |
| `guides/` | 使い方ガイド |

Living Documentation 原則: ドキュメントとコードは常に同期させる。
