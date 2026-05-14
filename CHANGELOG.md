# 変更履歴

このプロジェクトの主要な変更はこのファイルに記録されます。

フォーマットは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に基づいており、
このプロジェクトは [セマンティックバージョニング](https://semver.org/lang/ja/) に準拠しています。

## [0.4.2] - 2026-05-14

### 修正
- QUIC channel handler の正常 close (EOF) を ERROR から DEBUG に degrade ([#30](https://github.com/chronista-club/unison/pull/30))
  - 正常終端の `NetworkError::Protocol("Channel closed" | "Raw channel closed" | "Request cancelled: channel closed")` が ERROR ログされていた問題を解消
  - fleetstage prod で 24h 5739 件の偽 ERROR ノイズを発生させていた base 要因を除去

### 追加
- `NetworkError::is_normal_close()` helper メソッド
  - 3 種類の正常 channel 終端 (`recv` / `recv_raw` / `request`) を判定
  - 文字列マッチで暫定実装 (将来 `NetworkError::ChannelEof` enum variant 化予定 — USN-5)
- Channel lifecycle ログの対称化: open 側も `debug!` で記録 (close 側と対応)

### 内部
- 設計ヒアリングを Linear に集約 (USN-1〜5)
- Hierophant Green 💚 KDL schema を `schemas/hierophant.kdl` に定義 (USN-3 Phase 1)
- `unison-mcp-probe` crate を追加: Claude Code から Unison サーバを対話的につつく MCP tool 群 (USN-2)

## [0.4.1] - 2026-04-25

### 追加
- QUIC が DNS hostname と IPv4 リテラルを受け付けるように拡張 ([#29](https://github.com/chronista-club/unison/pull/29))
  - `parse_ipv6_address` → `resolve_socket_addr` (async, `tokio::net::lookup_host` ベース)
  - URL scheme strip (`https://` / `http://` / `quic://`)
  - 9 件の unit test 追加 (IPv4 / IPv6 / hostname / scheme / unresolvable)

### 後方互換
- 既存 `[ipv6]:port` / `::1` / `8080` / `localhost:port` 経路は全て維持 (additive)

## [0.4.0] - 2026-04-19

### 追加
- Codec トレイト + buffa (protobuf) 統合
  - `UnisonChannel<C: Codec>` で JSON / protobuf を差し替え可能に
  - `JsonCodec` (`serde::Serialize` / `DeserializeOwned`) と `ProtoCodec` (`buffa::Message`) を提供

## [0.3.0] - 2026-02-20

### 追加
- `ServerHandle`: `spawn_listen()` によるバックグラウンド起動とグレースフルシャットダウン
  - `shutdown()`: グレースフルシャットダウン
  - `is_finished()`: 終了状態の確認
  - `local_addr()`: バインドアドレスの取得
- `ConnectionEvent`: 接続/切断のリアルタイム通知
  - `Connected { remote_addr, context }` / `Disconnected { remote_addr }`
  - `subscribe_connection_events()` で購読
- Raw bytes チャネルサポート: rkyv/zstd をバイパスした最小オーバーヘッドのバイナリ通信
  - `UnisonChannel::send_raw()` / `recv_raw()`
  - Typed Frame フォーマット: `[4B length][1B type tag][payload]`（0x00=Protocol, 0x01=Raw）
- `UnisonStream::send_frame()` / `recv_frame()` / `recv_typed_frame()`: フレームベースの直接 I/O
- `UnisonStream::close_stream()`: `&self` で呼べるストリームクローズ

### 修正
- チャネル通信の二重ラッピングバグを修正（ProtocolMessage が二重にネストされていた）
- `SystemStream::receive()` の `read_to_end` 問題を修正（マルチメッセージ通信が不可能だった）
- `UnisonChannel` のストリーム参照を `Arc<Mutex<UnisonStream>>` → `Arc<UnisonStream>` に簡素化

### 変更
- チャネル内部の送受信を `SystemStream` 経由から直接フレーム I/O に移行
- README.md を v0.2 以降の現状に合わせて全面更新

## [0.2.0] - 2026-02-16

### 追加
- `UnisonChannel`: 統合チャネル型（request/response + event push）
  - `request()`: Request/Response パターン（メッセージID自動生成、pending管理）
  - `send_response()`: サーバー側 Response 送信
  - `send_event()`: 一方向 Event 送信
  - `recv()`: メッセージ受信（Request/Event）
  - 内部 recv ループによる自動振り分け（Response → pending oneshot、その他 → event queue）
- KDL スキーマに `request` / `returns` / `event` 構文を追加
  - `ChannelRequest` / `ChannelEvent` パーサー構造体
- `CLAUDE.md`: プロジェクト開発方針ドキュメント
- Identity Channel: `ServerIdentity` によるリアルタイム自己紹介
- `ConnectionContext`: 接続状態管理（チャネルハンドル、Identity）

### 変更
- **Unified Channel アーキテクチャ**: RPC を全廃し、全通信をチャネルに統一
- `MessageType`: 10 variants → 4 に簡素化（`Request`, `Response`, `Event`, `Error`）
- `ProtocolServer`: `register_handler()` → `register_channel()` に移行
- `ProtocolClient`: `call()` 削除、`open_channel()` → `UnisonChannel` を返す
- KDL スキーマ: `service`/`method` → `channel`/`request`/`event` 構文に移行
- Rust コード生成: `UnisonChannel` ベースに更新
- TypeScript コード生成: `call()` → `request()` に統一
- Examples / Tests / Benchmarks を全て channel ベースに書き換え
- 仕様ドキュメント（spec/01〜03）を Unified Channel に全面書き換え
- 設計ドキュメント（design/）を UnisonChannel アーキテクチャに更新

### 削除
- `register_handler()` / `call()` / `open_typed_channel()` — 旧 RPC メソッド
- `QuicBackedChannel<S, R>` / `StreamSender` / `StreamReceiver` / `BidirectionalChannel` — 未使用型
- `ProtocolClientTrait` / `ProtocolServerTrait` / `UnisonServerExt` / `UnisonClientExt` — 旧トレイト
- `MessageType` の 7 deprecated variants（Stream系）
- `process_message()` / `handle_call()` — 旧 RPC サーバー処理
- `send_response()` (quic.rs 内の dead code)

## [0.1.0-alpha3] - 2025-10-21

### 追加
- 新しい`frame`モジュールの実装
  - `UnisonFrame`構造体でヘッダー、ペイロード、フラグ、設定を統合管理
  - `RkyvPayload`によるゼロコピーシリアライゼーション
  - Zstd圧縮とCRC32チェックサム機能
  - フレームベースの通信プロトコル
- `.claude/skills/developer.md`を追加して開発規約を整理
- `design/packet.md`を追加してパケット仕様を文書化

### 変更
- パーサーをknuffelに完全移行
  - KDLスキーマパーシングをknuffelベースに統一
  - インラインメソッド定義をサポート（`MethodMessage`型）
- ネットワーク層を`UnisonFrame<RkyvPayload<ProtocolMessage>>`を使用するように統合
- `packetモジュールをframeモジュールにリネーム
- テストコードを`new_with_json()`メソッドに統一
- WebSocketモジュールを削除（QUICに集中）

### 改善
- CI/CDの強化
  - Windows環境でのPDB制限エラーを回避（codegen-units増加）
  - macOS環境でのリンカーシンボル長制限に対応
  - Clippy警告を修正してCI通過を実現
- ドキュメント整理
  - 英語版ドキュメントを削除して日本語版に集約
  - 不要なファイルを削除（CONTRIBUTING.ja.md、SECURITY.ja.md等）
- 依存関係の更新
  - MSRV（Minimum Supported Rust Version）を1.85に更新
  - `cargo-deny` 0.18フォーマットに対応
  - knuffelをフォーク版（chronista-club/knuffel）に変更

### 修正
- パケットビルダーでチェックサムが正しく有効化されるように修正
- CI環境でのリンカーエラーを修正
- フォーマットとベンチマークのAPIミスマッチを修正
- スキーマパーステストを簡略化

## [0.1.0] - 2025-01-05

### 追加
- 🎵 QUICトランスポートを採用したUnison Protocolの初期リリース
- 型安全な通信のためのKDLベースのスキーマ定義システム
- 超低遅延トランスポートを備えたQUICクライアントとサーバー実装
- 包括的な型検証とコード生成を備えたスキーマパーサー
- Quinn + rustlsを使用したTLS 1.3対応の最新QUICトランスポート層
- 自動証明書生成とプロダクション用rust-embedサポート
- コアプロトコル型: `UnisonMessage`, `UnisonResponse`, `NetworkError`
- `UnisonClient`, `UnisonServer`, `UnisonServerExt` トレイトによるネットワーク抽象化
- 完全なドキュメントとQUICプロトコル仕様
- 実装例:
  - `unison_ping_server.rs` - ハンドラー登録機能を備えたQUICベースのping-pongサーバー
  - `unison_ping_client.rs` - レイテンシ測定付き高性能QUICクライアント
- スキーマ定義:
  - `unison_core.kdl` - コアUnisonプロトコルスキーマ
  - `ping_pong.kdl` - 複数メソッドを含むping-pongプロトコル例
  - `diarkis_devtools.kdl` - 開発ツール用の高度なプロトコル
- 包括的なテストスイート:
  - `simple_quic_test.rs` - QUIC機能と証明書テスト
  - `quic_integration_test.rs` - 完全なクライアント・サーバー統合テスト
- `build.rs`による自動証明書生成ビルドシステム
- オープンソース配布用MITライセンス

### 機能
- **型安全性**: KDLスキーマによるコンパイル時と実行時のプロトコル検証
- **QUICトランスポート**: TLS 1.3暗号化による超低遅延通信
- **マルチストリームサポート**: 単一接続での効率的な並列通信
- **ゼロコンフィギュレーション**: 開発環境用の自動証明書生成
- **プロダクション対応**: バイナリ内の組み込み証明書用rust-embedサポート
- **スキーマ検証**: 包括的な検証を備えたKDLベースのプロトコル定義
- **コード生成**: 自動クライアント/サーバーコード生成（Rust完成、TypeScript予定）
- **非同期ファースト**: 高性能非同期I/Oとfutures用にtokioで構築
- **包括的テスト**: 完全なクライアント・サーバーシナリオの単一プロセス統合テスト
- **開発者体験**: tracingによるリッチなログ、エラー処理、デバッグサポート

### 技術詳細
- **コア依存関係**: 
  - `quinn` 0.11+ - QUICプロトコル実装
  - `rustls` 0.23+ - ring暗号によるTLS 1.3暗号化
  - `tokio` 1.40+ - フル機能付き非同期ランタイム
  - `kdl` 4.6+ - スキーマ解析と検証
  - `serde` 1.0+ - derive機能付きJSONシリアライゼーション
  - `rcgen` 0.13+ - 自動証明書生成
  - `rust-embed` 8.5+ - バイナリへの証明書埋め込み
  - `Cargo.toml`に完全な依存関係リストと機能
- **ビルドシステム**: 証明書自動生成とコード生成を備えたカスタムビルドスクリプト
- **テスト**: 包括的なユニットテスト、QUIC統合テスト、パフォーマンス検証
- **ドキュメント**: 完全なAPIドキュメント、使用例、QUICプロトコル仕様
- **セキュリティ**: デフォルトでTLS 1.3、自動証明書管理、セキュアなデフォルト設定

### リポジトリ構造
```
unison/
├── .github/workflows/ci.yml    # GitHub Actions CI with Rust matrix testing
├── .gitignore                  # Git ignore rules
├── Cargo.toml                  # Rust package with QUIC dependencies
├── LICENSE                     # MIT License
├── README.md                   # Updated QUIC-focused documentation
├── CHANGELOG.md                # This file
├── build.rs                    # Build script with certificate generation
├── src/                        # Source code
│   ├── lib.rs                  # Library entry point with QUIC exports
│   ├── core/                   # Core protocol types and traits
│   ├── parser/                 # KDL schema parsing with validation
│   ├── codegen/                # Code generation for Rust and TypeScript
│   └── network/                # QUIC implementation
│       ├── mod.rs              # Network traits and error types
│       ├── client.rs           # QUIC client implementation
│       ├── server.rs           # QUIC server with handler registration
│       └── quic.rs             # QUIC transport with Quinn/rustls
├── assets/                     # Build-time generated assets
│   └── certs/                  # Auto-generated QUIC certificates
│       ├── cert.pem            # Server certificate
│       └── private_key.der     # Private key
├── schemas/                    # Protocol schema definitions
│   ├── unison_core.kdl         # Core protocol schema
│   ├── ping_pong.kdl           # Example ping-pong with multiple methods
│   └── diarkis_devtools.kdl    # Advanced development tools protocol
├── tests/                      # Integration tests
│   ├── simple_quic_test.rs     # QUIC functionality tests
│   └── quic_integration_test.rs # Full client-server integration
├── examples/                   # Usage examples
│   ├── unison_ping_server.rs   # QUIC server with handler registration
│   └── unison_ping_client.rs   # QUIC client with performance metrics
└── docs/                       # Documentation
    ├── README.md               # Japanese documentation
    ├── README-en.md            # English documentation  
    └── PROTOCOL_SPEC_ja.md     # QUIC protocol specification
```

### パフォーマンス特性
- **接続**: 超高速接続確立
- **レイテンシ**: 超低遅延通信
- **スループット**: マルチストリーミングによる高スループット
- **セキュリティ**: TLS 1.3暗号化とforward secrecy
- **リソース**: CPU/メモリ使用量の最適化

### 今後の予定（ロードマップ）
- [ ] crates.ioへ `unison` v0.1.0 として公開
- [ ] WebTransport APIサポート付きTypeScript/JavaScriptコード生成
- [ ] aioquic統合によるPythonバインディング
- [ ] quic-go統合によるGoバインディング
- [ ] カスタムバリデータによる拡張スキーマ検証
- [ ] パフォーマンスベンチマークと最適化分析
- [ ] ロードバランシングとコネクションマイグレーション機能
- [ ] 大規模データ転送のためのストリーミングサポート

### 移行に関する注意
これはQUICトランスポートを主要プロトコルとした初期の独立リリースです。このフレームワークは、優れたパフォーマンスとセキュリティ特性を活用し、QUIC通信専用に設計されています。

### 既知の問題
- 本番環境での証明書検証には適切なCA署名済み証明書が必要
- 一部の企業ファイアウォールはQUICに必要なUDPトラフィックをブロックする可能性
- WebTransport APIのサポートはブラウザにより異なる（Chrome 97+、Firefox実験的）

### コミュニティとサポート
- GitHub Issues: バグ報告と機能リクエスト
- GitHub Discussions: コミュニティサポートと質問  
- ドキュメント: `docs/` ディレクトリ内の包括的なガイド
- 例: `examples/` 内の本番対応サーバー/クライアント実装