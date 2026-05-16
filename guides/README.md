# 実装ガイド（Guides）

実装時に参考にするためのガイドやチュートリアルです。

## 目的

「どう使うか」を説明します。

- ステップバイステップのチュートリアル
- コードサンプル
- よくある問題と解決方法
- 外部ライブラリの使い方

## ガイド一覧

| ガイド | 説明 |
|-------|------|
| [integration-guide.md](integration-guide.md) | 呼び出し側プロジェクトへ Unison 統合を組み込むためのオンボーディング（ここから始める） |
| [quickstart.md](quickstart.md) | Unison サーバ + TypeScript クライアントを end-to-end で疎通させる |
| [migration.md](migration.md) | v1.0 までの破壊的変更と移行手順 |
| [typescript-sdk.md](typescript-sdk.md) | TypeScript クライアント SDK の API リファレンス |
| [channel-guide.md](channel-guide.md) | Rust 側 UnisonChannel API の実践ガイド |
| [quinn-stream-api.md](quinn-stream-api.md) | Quinn（QUIC実装）のストリームAPIの使い方 |

## ガイドの書き方

1. **目的を明確に**: このガイドで何ができるようになるか
2. **ステップバイステップ**: 順を追って説明
3. **実践的なコード例**: コピペで動くサンプルコード
4. **よくある問題**: ハマりやすいポイントと解決方法
5. **参考リンク**: 公式ドキュメント等へのリンク

## 更新方針

- 実装方法が変わったら更新
- ユーザーからのフィードバックを反映
- 実際に動作するコード例を維持

## 関連ドキュメント

- [仕様書](../spec/) - 何を実現するか
- [設計ドキュメント](../design/) - どう実装するか
- [開発者スキル](../.claude/developer.md) - コーディング規約
