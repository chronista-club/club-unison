# 設計ドキュメント（Design）

仕様をどう実装するかの詳細設計を記述します。

## 目的

「どう実装するか」を説明します。

- 実装アプローチ
- データ構造とアルゴリズム
- コンポーネント間の関係
- パフォーマンス考慮事項

## 設計ドキュメント一覧

| ドキュメント | 説明 | 対応仕様 |
|------------|------|----------|
| [architecture.md](architecture.md) | 全体アーキテクチャ設計詳細 | [spec/01](../spec/01-core-concept/SPEC.md) |
| [packet.md](packet.md) | UnisonPacket実装仕様（バイナリパケット層） | [spec/01](../spec/01-core-concept/SPEC.md) |
| [wire-format.md](wire-format.md) | Wire Format 設計（v0.9.0 buffa pivot） | — |
| [kdl-to-json-schema.md](kdl-to-json-schema.md) | KDL 型 → JSON Schema 対応表（unison-mcp の tool schema） | [spec/02](../spec/02-unified-channel/SPEC.md) §4.1 |
| [quic-runtime.md](quic-runtime.md) | QUIC Runtime 統合 | — |
| [connection-auth.md](connection-auth.md) | Connection-level auth primitive（mechanism/policy 分離） | — |
| [datagram-channel.md](datagram-channel.md) | Datagram Channel 設計（best-effort lane、v0.10.0） | — |
| [server-initiated-stream.md](server-initiated-stream.md) | Server-initiated reliable stream（`ServerToClient` を起こす、ADR-020 §S4） | — |
| [happy-eyeballs-dial.md](happy-eyeballs-dial.md) | `connect_race` — Happy Eyeballs v2 staggered-race dialer（ADR-020 §S6） | — |
| [swift-client-api.md](swift-client-api.md) | Swift Client SDK API 設計 | — |
| [typescript-client-api.md](typescript-client-api.md) | TypeScript Client SDK API 設計（v1.0 Phase 3a） | — |
| [test-strategy.md](test-strategy.md) | テスト戦略（3x3マトリクス） | — |

`review/` は特定時点のレビュー記録 (living doc ではない)。

## 設計ドキュメントの書き方

1. **対応する仕様へのリンク**: どの仕様を実装しているか明示
2. **実装の詳細**: データ構造、アルゴリズム、コンポーネント設計
3. **コード例**: 実装の具体例を含める
4. **パフォーマンス考慮事項**: メモリ使用量、処理速度等
5. **実装時の注意点**: ハマりやすいポイント、制約事項

## 更新方針

- 実装の変更に合わせて積極的に更新
- PRレビュー時に関連ドキュメントの更新も確認
- 実装とドキュメントの整合性を保つ

## 関連ドキュメント

- [仕様書](../spec/) - 何を実現するか
- [実装ガイド](../guides/) - どう使うか
