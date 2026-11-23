---
skill: developer
description: Unisonプロジェクト開発時のコーディング規約、アーキテクチャ、実装パターンのガイド
tags: [rust, coding-standards, architecture, best-practices]
---

# Unison開発者スキル

このスキルは、Unisonプロトコルの開発に携わる際の、コーディング規約、アーキテクチャパターン、実装ガイドラインを提供します。

## プロジェクト技術スタック

### コア技術
- **言語**: Rust 2024 Edition (MSRV: 1.85)
- **トランスポート**: QUIC (quinn)
- **シリアライゼーション**: rkyv（ゼロコピー）、serde_json（互換性）
- **圧縮**: zstd Level 1（2KB以上で自動適用）
- **スキーマ定義**: KDL (KDL Document Language)
- **非同期ランタイム**: Tokio

### 設計原則
1. **型安全性優先**: コンパイル時の型チェックを最大限活用
2. **ゼロコピー最適化**: メモリアロケーションを最小限に
3. **自動圧縮**: 帯域効率を透過的に最適化
4. **拡張可能**: トレイトベースの抽象化で柔軟な拡張
5. **日本語ファースト**: ドキュメント・コメントは日本語で統一

## アーキテクチャ構造

### レイヤー構造

```
┌─────────────────────────────────────┐
│     アプリケーション層              │
│  (Services, Handlers, Business Logic)│
├─────────────────────────────────────┤
│        プロトコル層                  │
│  (Schema Parser, Code Generation)    │
├─────────────────────────────────────┤
│        パケット層                    │
│  (UnisonPacket, Serialization)       │
├─────────────────────────────────────┤
│      トランスポート層                │
│  (QUIC, Stream Management)           │
└─────────────────────────────────────┘
```

### モジュール構成

```
src/
├── packet/         # バイナリパケット層
│   ├── mod.rs     # UnisonPacket<T>実装
│   ├── header.rs  # 64バイト固定長ヘッダー
│   ├── flags.rs   # 16ビットフラグ管理
│   ├── payload.rs # Payloadableトレイト
│   └── serialization.rs # rkyv/zstd処理
│
├── network/       # ネットワーク層
│   ├── quic.rs   # QUICトランスポート
│   ├── client.rs # クライアント実装
│   ├── server.rs # サーバー実装
│   └── service.rs # サービス抽象化
│
├── parser/        # KDLスキーマパーサー
├── codegen/       # コード生成（Rust/TypeScript）
├── context/       # CGPコンテキスト
└── core/         # コア型定義
```

## コーディング規約

### 命名規則

- **構造体/トレイト**: PascalCase（例: `UnisonPacket`）
- **関数/メソッド**: snake_case（例: `to_bytes`）
- **定数**: SCREAMING_SNAKE_CASE（例: `COMPRESSION_THRESHOLD`）
- **型パラメータ**: 単一大文字（例: `T: Payloadable`）

### ドキュメント規約

```rust
/// UnisonPacketのヘッダー構造
/// 
/// 固定長64バイトのヘッダーで、パケットのメタデータを格納します。
/// 
/// # フィールド
/// - `version`: プロトコルバージョン（現在: 0x01）
/// - `packet_type`: パケットタイプ識別子
/// - `flags`: 16ビットフラグ
pub struct UnisonPacketHeader {
    pub version: u8,
    pub packet_type: u8,
    pub flags: u16,
    // ...
}
```

### エラーハンドリング

```rust
// Result型を使用した適切なエラー伝播
pub fn process_packet(data: &[u8]) -> Result<UnisonPacket, PacketError> {
    let header = UnisonPacketHeader::from_bytes(data)?;
    
    if !header.is_valid() {
        return Err(PacketError::InvalidHeader);
    }
    
    Ok(UnisonPacket::from_parts(header, payload)?)
}

// パニックは避ける
// ❌ panic!("Invalid packet!");
// ✅ return Err(PacketError::InvalidPacket);
```

### 非同期処理

```rust
// async/awaitを使用
pub async fn send_packet(&mut self, packet: UnisonPacket) -> Result<(), NetworkError> {
    let bytes = packet.to_bytes()?;
    self.stream.write_all(&bytes).await?;
    Ok(())
}

// Arc<RwLock<T>>での状態共有
use tokio::sync::RwLock;
let state = Arc::new(RwLock::new(ServerState::new()));

// 並行処理
tokio::spawn(async move {
    // 並行タスク
});
```

## 実装パターン

### 1. ビルダーパターン

```rust
impl<T: Payloadable> UnisonPacketBuilder<T> {
    pub fn new() -> Self { 
        Self::default()
    }
    
    pub fn with_stream_id(mut self, id: u64) -> Self {
        self.header.stream_id = id;
        self
    }
    
    pub fn with_sequence(mut self, seq: u64) -> Self {
        self.header.sequence_number = seq;
        self
    }
    
    pub fn build(self, payload: T) -> Result<UnisonPacket<T>, PacketError> {
        Ok(UnisonPacket::from_builder(self, payload)?)
    }
}
```

### 2. ゼロコピーデシリアライゼーション

```rust
// rkyvを使用した効率的な読み取り
pub fn from_bytes_zero_copy(bytes: &[u8]) -> Result<&Self::Archived, PayloadError> {
    let archived = rkyv::check_archived_root::<Self>(bytes)
        .map_err(|e| PayloadError::Deserialization(e.to_string()))?;
    // archivedは元のbytesを参照（コピーなし）
    Ok(archived)
}
```

### 3. 自動圧縮

```rust
const COMPRESSION_THRESHOLD: usize = 2048; // 2KB
const COMPRESSION_LEVEL: i32 = 1;

fn maybe_compress(payload: &[u8]) -> Vec<u8> {
    if payload.len() >= COMPRESSION_THRESHOLD {
        let compressed = zstd::encode_all(payload, COMPRESSION_LEVEL)
            .expect("Compression failed");
        
        if compressed.len() < payload.len() {
            return compressed; // 圧縮が効果的
        }
    }
    payload.to_vec() // 圧縮しない
}
```

## テスト戦略

### ユニットテスト

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    
    #[test]
    fn test_packet_serialization() {
        let payload = StringPayload::from_string("test");
        let packet = UnisonPacket::builder()
            .with_stream_id(1)
            .build(payload)
            .unwrap();
        
        let bytes = packet.to_bytes().unwrap();
        let decoded = UnisonPacket::<StringPayload>::from_bytes(&bytes).unwrap();
        
        assert_eq!(decoded.header().stream_id, 1);
    }
}
```

### 統合テスト

```rust
// tests/integration_test.rs
#[tokio::test]
async fn test_client_server_roundtrip() {
    let server = TestServer::start().await;
    let mut client = TestClient::connect(server.addr()).await.unwrap();
    
    let request = create_test_packet();
    client.send(request).await.unwrap();
    
    let response = client.receive().await.unwrap();
    assert!(response.is_success());
}
```

## 依存関係の選定理由

| クレート | バージョン | 用途 | 選定理由 |
|---------|-----------|------|----------|
| quinn | 0.11 | QUICトランスポート | 最も成熟したRust QUIC実装 |
| rkyv | 0.7 | シリアライゼーション | ゼロコピーで最高速 |
| zstd | 0.13 | 圧縮 | リアルタイム圧縮に最適 |
| bytes | 1.10 | バッファ管理 | 効率的なバイト操作 |
| tokio | 1.40 | 非同期ランタイム | デファクトスタンダード |
| serde | 1.0 | JSON互換性 | エコシステム標準 |

## パフォーマンス目標

### レイテンシー
- パケット処理: < 1μs（非圧縮）
- 圧縮（2KB）: < 5μs
- QUIC接続確立: < 100ms

### スループット
- 単一ストリーム: > 1Gbps
- 並行ストリーム: > 10,000/秒
- パケット処理: > 1M packets/秒

### メモリ使用量
- パケットヘッダー: 64 bytes固定
- ゼロコピー読み取り: 追加アロケーションなし
- バッファプール: 事前割り当て

## デバッグガイド

### ログ出力

```bash
# 詳細ログ有効化
RUST_LOG=debug cargo run

# 特定モジュールのみ
RUST_LOG=unison::packet=trace cargo run

# 複数モジュール
RUST_LOG=unison::packet=trace,unison::network=debug cargo run
```

### よくある問題と解決方法

#### 1. rkyvのバージョン不一致
**問題**: `AllocSerializer`関連のコンパイルエラー

**解決**:
```toml
[dependencies]
rkyv = { version = "0.7", features = ["validation"] }
```

#### 2. ライフタイムエラー
**問題**: ゼロコピー実装でのライフタイムエラー

**解決**:
```rust
// ❌ 所有権を失う
let archived = packet.payload_zero_copy();
drop(packet); // archivedが無効に

// ✅ バッファを保持
let bytes = packet.to_bytes();
let archived = Payload::from_bytes_zero_copy(&bytes);
```

#### 3. 非同期デッドロック
**問題**: `RwLock`のデッドロック

**解決**:
```rust
// ❌ .awaitを跨いでロック保持
let guard = state.write().await;
some_async_operation().await; // デッドロックのリスク

// ✅ ロックを早めに解放
{
    let mut guard = state.write().await;
    guard.update();
} // guardがここでドロップ
some_async_operation().await;
```

## コミット規約

### コミットメッセージフォーマット

```
<type>: <subject>

<body>

<footer>
```

### Type
- `feat`: 新機能追加
- `fix`: バグ修正
- `docs`: ドキュメント更新
- `test`: テスト追加・修正
- `refactor`: リファクタリング
- `perf`: パフォーマンス改善
- `chore`: ビルド・ツール関連

### 例

```
feat: UnisonPacketにmessage_idフィールドを追加

Request/Response相関のためにmessage_idとresponse_toフィールドを追加。
これにより、ペイロードを見ずにメッセージタイプを識別可能になる。

Closes #42
```

## プルリクエストガイドライン

1. **ブランチ命名**: `feature/機能名`, `fix/問題名`
2. **フォーマット**: `cargo fmt`実行
3. **Lint**: `cargo clippy`で警告解消
4. **テスト**: カバレッジ80%以上
5. **説明**: 日本語でPR説明記載

## コードレビューチェックリスト

### メモリ安全性
- [ ] 不正な参照やリークがないか
- [ ] ライフタイムが適切に指定されているか
- [ ] 所有権の移動が明確か

### エラー処理
- [ ] パニックせずResult型で処理しているか
- [ ] エラーが適切に伝播されているか
- [ ] エラーメッセージが分かりやすいか

### 並行性
- [ ] データ競合のリスクがないか
- [ ] デッドロックの可能性がないか
- [ ] ロックの粒度が適切か

### 効率性
- [ ] 不要なアロケーションがないか
- [ ] 不要なコピーがないか
- [ ] ゼロコピーが活用されているか

### 可読性
- [ ] 適切な抽象化レベルか
- [ ] 命名が明確か
- [ ] ドキュメントコメントが適切か

## AI支援時の推奨事項

### コード生成時
1. **必ずエラーハンドリングを含める**
2. **ライフタイムは明示的に指定**
3. **ドキュメントコメントは日本語で**
4. **テストコードも同時に生成**
5. **パフォーマンスを意識した実装**

### 質問への回答方針
1. **具体的なコード例を提示**
2. **パフォーマンスへの影響を説明**
3. **代替案がある場合は比較**
4. **既存コードとの整合性を重視**
5. **Rustのベストプラクティスに従う**

## 参考リンク

- [仕様書](../spec/) - プロジェクト仕様
- [設計ドキュメント](../design/) - 実装設計詳細
- [実装ガイド](../guides/) - 実装時の参考資料
- [The Rust Book](https://doc.rust-lang.org/book/) - Rust公式ガイド
- [Tokio Tutorial](https://tokio.rs/tokio/tutorial) - Tokio公式チュートリアル
