# Unison Protocol への貢献

Unison Protocol への貢献に興味を持っていただきありがとうございます！コミュニティからの貢献を歓迎し、どんな形での支援も感謝しています。

## 目次

- [行動規範](#行動規範)
- [はじめに](#はじめに)
- [貢献方法](#貢献方法)
- [開発プロセス](#開発プロセス)
- [プルリクエストプロセス](#プルリクエストプロセス)
- [スタイルガイドライン](#スタイルガイドライン)
- [テスト](#テスト)
- [ドキュメント](#ドキュメント)
- [コミュニティ](#コミュニティ)

## 行動規範

このプロジェクトとその参加者は、[行動規範](CODE_OF_CONDUCT.ja.md)に従います。参加することで、この規範を守ることが期待されます。受け入れがたい行動は contact@chronista.club に報告してください。

## はじめに

### 前提条件

- Rust 1.70 以上
- Tokio 1.40 以上
- OpenSSL または BoringSSL（QUIC サポート用）

### 開発環境のセットアップ

1. GitHub でリポジトリをフォーク
2. フォークをローカルにクローン：
   ```bash
   git clone https://github.com/your-username/unison.git
   cd unison
   ```

3. アップストリームリポジトリをリモートとして追加：
   ```bash
   git remote add upstream https://github.com/chronista-club/club-unison.git
   ```

4. **(macOS のみ)** LLD リンカーをインストールして設定：
   ```bash
   brew install lld
   ```
   
   インストール後、プロジェクトルートに `.cargo/config.toml` ファイルを作成して以下の設定を追加：
   ```toml
   [target.aarch64-apple-darwin]
   linker = "clang"
   rustflags = ["-C", "link-arg=-fuse-ld=/opt/homebrew/bin/ld64.lld"]
   ```
   
   > **注意**: macOS では標準リンカーに制限があるため、テストを実行するには `lld` リンカーが必要です。
   > `.cargo/config.toml` はローカル開発環境専用の設定ファイルです（`.gitignore` に含まれています）。CI 環境では不要です。

5. プロジェクトをビルド：
   ```bash
   cargo build
   ```

6. テストを実行して動作確認：
   ```bash
   cargo test
   ```

## 貢献方法

### バグ報告

バグレポートを作成する前に、重複を避けるため既存の Issue を確認してください。バグレポートを作成する際は、できるだけ詳細に記載してください：

- 明確で説明的なタイトルを使用
- 問題を再現する正確な手順を記述
- 手順を示す具体例を提供
- 観察された動作と、それが問題である理由を説明
- 期待される動作を説明
- 環境の詳細（OS、Rust バージョンなど）を含める

### 機能提案

機能提案は GitHub Issue として管理されます。機能提案を作成する際は：

- 明確で説明的なタイトルを使用
- 提案する機能の詳細な説明を提供
- 機能の使用例を含める
- なぜこの機能が多くのユーザーに有用かを説明

### 初めてのコード貢献

どこから始めればよいか分からない場合は、以下のラベルが付いた Issue を探してください：

- `good first issue` - 初心者向け
- `help wanted` - 追加の支援が必要
- `documentation` - ドキュメントの改善

## 開発プロセス

### ブランチ戦略

- `main` - メイン開発ブランチ
- フィーチャーブランチは `main` から作成
- 説明的なブランチ名を使用：`feature/add-new-handler`、`fix/connection-timeout` など

### コミットメッセージ

[Conventional Commits](https://www.conventionalcommits.org/) 仕様に従います：

```
<type>(<scope>): <subject>

<body>

<footer>
```

タイプ：
- `feat`: 新機能
- `fix`: バグ修正
- `docs`: ドキュメント変更
- `style`: コードスタイル変更（フォーマットなど）
- `refactor`: コードリファクタリング
- `perf`: パフォーマンス改善
- `test`: テストの追加または更新
- `chore`: メンテナンスタスク

例：
```
feat(network): QUIC接続のリトライロジックを追加
fix(parser): KDLパースのエッジケースを処理
docs: UnisonStreamのAPIドキュメントを更新
```

## プルリクエストプロセス

1. コードがプロジェクトのスタイルガイドラインに準拠していることを確認
2. 必要に応じてドキュメントを更新
3. 新機能にテストを追加
4. すべてのテストが合格することを確認：`cargo test`
5. フォーマットを実行：`cargo fmt`
6. リンティングを実行：`cargo clippy`
7. 変更を CHANGELOG.md に記載（該当する場合）
8. 明確なタイトルと説明でプルリクエストを作成

### PR レビュープロセス

- 少なくとも1人のメンテナーによるレビューが必要
- すべての CI チェックが合格する必要がある
- コードカバレッジが低下しないこと
- 新機能にはドキュメントの更新が必要

## スタイルガイドライン

### Rust コードスタイル

- 標準的な Rust の慣習とイディオムに従う
- `cargo fmt` でコードをフォーマット
- `cargo clippy` で一般的なミスをキャッチ
- `unwrap()` より明示的なエラー処理を優先
- 説明的な変数名と関数名を使用
- 複雑なロジックにはコメントを追加

### ドキュメントスタイル

- 明確で簡潔な言語を使用
- 適切な場所にコード例を含める
- README と他のドキュメントを最新に保つ
- すべてのパブリック API をドキュメント化

## テスト

### テストの実行

```bash
# すべてのテストを実行
cargo test

# 特定のテストを実行
cargo test test_name

# 出力付きでテストを実行
RUST_LOG=debug cargo test -- --nocapture

# 統合テストを実行
cargo test --test quic_integration_test
```

### テストの作成

- すべての新機能に単体テストを作成
- 複雑な機能には統合テストを含める
- 少なくとも 80% のコードカバレッジを目指す
- エッジケースとエラー条件をテスト

### ベンチマーク

```bash
# ベンチマークを実行
cargo bench

# 特定のベンチマークを実行
cargo bench bench_name
```

## ドキュメント

- Rust ドキュメントコメントを使用してすべてのパブリック API をドキュメント化
- ドキュメントに例を含める
- README を最新に保つ
- 重要な変更にはアーキテクチャドキュメントを更新

### ドキュメントのビルド

```bash
# ドキュメントをビルドして開く
cargo doc --open

# プライベートアイテムを含むドキュメントをビルド
cargo doc --document-private-items
```

## コミュニティ

### コミュニケーションチャンネル

- GitHub Issues: バグレポートと機能リクエスト
- GitHub Discussions: 一般的な議論と Q&A
- Discord: [Discord サーバーに参加](https://discord.gg/unison)（準備中）

### ヘルプを得る

ヘルプが必要な場合：

1. [ドキュメント](https://docs.rs/unison)を確認
2. 既存の Issue を検索
3. GitHub Discussions で質問
4. Discord でお問い合わせ

## 謝辞

貢献者は以下で認識されます：
- プロジェクトの CHANGELOG.md
- リリースノートでの特別な言及
- 貢献者リスト

## ライセンス

Unison Protocol に貢献することで、あなたの貢献が MIT ライセンスの下でライセンスされることに同意したものとみなされます。

---

Unison Protocol への貢献ありがとうございます！あなたの努力により、このプロジェクトはすべての人にとってより良いものになります。🎵