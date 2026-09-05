# チャネルガイド

Unison Protocol の UnisonChannel を使った通信の実践ガイド。

---

## 1. はじめに

UnisonChannel は Unison Protocol の統合チャネル型。各チャネルは独立した QUIC ストリームにマッピングされ、Head-of-Line Blocking を回避しながら request/response と event push の両方をサポートする。

### 前提知識

- Rust の async/await 基礎
- Unison Protocol の基本概念（[spec/01](../spec/01-core-concept/SPEC.md) 参照）

---

## 2. クイックスタート

### 2.1 KDL スキーマでチャネル定義

```kdl
protocol "my-protocol" version="1.0.0" {
    namespace "com.example.myprotocol"

    channel "query" from="client" lifetime="persistent" {
        request "Query" {
            field "method" type="string" required=#true
            field "params" type="json"

            returns "Result" {
                field "data" type="json"
            }
        }

        event "QueryError" {
            field "code" type="string"
            field "message" type="string"
        }
    }
}
```

**属性の意味**:

| 属性 | 値 | 説明 |
|------|-----|------|
| `from` | `"server"` | サーバーが送信を開始 |
| `from` | `"client"` | クライアントが送信を開始 |
| `from` | `"either"` | 双方が送信可能 |
| `lifetime` | `"persistent"` | 接続中ずっと維持 |
| `lifetime` | `"transient"` | リクエスト単位で開閉 |

**メッセージ種別**:

| 種別 | 用途 |
|------|------|
| `request` + `returns` | Request/Response パターン（応答あり） |
| `event` | 一方向イベント（応答不要） |
| `send` / `recv` | 旧構文（非推奨） |

### 2.2 サーバー: register_channel

```rust
use unison::ProtocolServer;
use unison::network::UnisonChannel;

let server = ProtocolServer::with_identity(
    "my-server", "1.0.0", "com.example.myprotocol",
);

server.register_channel("query", |ctx, stream| async move {
    let mut channel = UnisonChannel::new(stream);

    loop {
        match channel.recv().await {
            Ok(msg) => {
                // request を処理して response を返す
                let result = process_query(&msg).await;
                channel.send_event("result", result).await?;
            }
            Err(_) => break,
        }
    }
    Ok(())
}).await;

server.listen("[::1]:8080").await?;
```

### 2.3 クライアント: open_channel

```rust
use unison::ProtocolClient;
use unison::network::UnisonChannel;

let mut client = ProtocolClient::new_default()?;
client.connect("[::1]:8080").await?;

// チャネルを開く → UnisonChannel が返る
let channel: UnisonChannel = client.open_channel("query").await?;

// Request/Response
let result = channel.request("Query", json!({"method": "search", "params": {}})).await?;

// Event 受信
let event = channel.recv().await?;
```

---

## 3. UnisonChannel API

```rust
impl UnisonChannel {
    /// Request/Response（メッセージIDで紐付け）
    pub async fn request(&self, method: &str, payload: Value) -> Result<Value, NetworkError>;

    /// 一方向イベント送信（応答不要）
    pub async fn send_event(&self, method: &str, payload: Value) -> Result<(), NetworkError>;

    /// イベント受信
    pub async fn recv(&mut self) -> Result<ProtocolMessage, NetworkError>;

    /// チャネルを閉じる
    pub async fn close(&mut self) -> Result<(), NetworkError>;
}
```

### 内部動作

- `request()`: メッセージIDを振り、`pending` マップに oneshot を登録。recv ループが Response を受信すると対応する oneshot に送信
- `send_event()`: Event 型メッセージを送信。応答を待たない
- `recv()`: recv ループが Event を `event_rx` に流す。アプリケーションはここから読み取る

---

## 4. エラーハンドリング

| エラー | 原因 | 対処 |
|--------|------|------|
| `NetworkError::NotConnected` | QUIC 接続が確立されていない | `connect()` を先に呼ぶ |
| `NetworkError::Quic(...)` | ストリーム開設失敗 | 接続状態を確認し再試行 |
| `NetworkError::Protocol(...)` | シリアライゼーション失敗 | メッセージ型が正しいか確認 |
| `NetworkError::HandlerNotFound` | チャネルハンドラー未登録 | サーバー側で `register_channel` を確認 |

---

## 関連ドキュメント

- [spec/02: Unified Channel プロトコル仕様](../spec/02-unified-channel/SPEC.md)
- [spec/01: コアコンセプト](../spec/01-core-concept/SPEC.md)

---

**最終更新**: 2026-02-16
