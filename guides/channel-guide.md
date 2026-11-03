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
| `send` / `recv` | 廃止。書いてもエラーにならず無視される（schema-lint も通る）ので使わない |

### 2.2 サーバー: register_channel

```rust
use std::sync::Arc;
use unison::ProtocolServer;
use unison::network::{MessageType, UnisonChannel};

let server = ProtocolServer::with_identity(
    "my-server", "1.0.0", "com.example.myprotocol",
);

server.register_channel("query", |_ctx, stream| async move {
    let channel: UnisonChannel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                // request を処理して response を返す
                let result = process_query(&msg).await;
                channel.send_response(msg.id, &msg.method, &result).await?;
            }
            Ok(_) => {}
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}).await;

Arc::new(server).listener("[::1]:8080").run().await?;
```

client の `request()` は、同じ `id` を持つ Response（`send_response()` で返したもの）を
受けて解決する。`send_event()` で返すと client 側では Event として `recv()` に届くだけで、
`request()` は `Timeout` になる。

### 2.3 クライアント: open_channel

```rust
use serde_json::json;
use unison::ProtocolClient;
use unison::network::UnisonChannel;

let client = ProtocolClient::insecure_localhost()?;
client.connect("[::1]:8080").await?;

// チャネルを開く → UnisonChannel が返る
let channel: UnisonChannel = client.open_channel("query").await?;

// Request/Response
let result: serde_json::Value = channel
    .request("Query", &json!({"method": "search", "params": {}}))
    .await?;

// Event 受信（サーバーが send_event で push するまで待つ）
let event = channel.recv().await?;
```

`insecure_localhost()` はサーバー証明書を検証しない dev 用の client で、接続先は
loopback に限られる。検証する場合は `QuicClient::builder()` で `TrustAnchors` を
指定し、`ProtocolClient::new` に渡す。

---

## 3. UnisonChannel API

`UnisonChannel<C: Codec = JsonCodec>`。既定の `JsonCodec` では、`Encodable<C>` は
`serde::Serialize`、`Decodable<C>` は `serde::de::DeserializeOwned` な型（`channel.rs`）。

```rust
impl<C: Codec> UnisonChannel<C> {
    /// UnisonStream から channel を組み立て、recv ループを起動する
    pub fn new(stream: UnisonStream) -> Self;

    /// request() のタイムアウトを変える（既定 30 秒）
    pub fn with_request_timeout(self, timeout: Duration) -> Self;

    /// Request/Response（メッセージIDで紐付け）
    pub async fn request<Req, Resp>(&self, method: &str, req: &Req) -> Result<Resp, NetworkError>
    where
        Req: Encodable<C>,
        Resp: Decodable<C>;

    /// 一方向イベント送信（応答不要）
    pub async fn send_event<T: Encodable<C>>(&self, method: &str, payload: &T) -> Result<(), NetworkError>;

    /// recv() で受けた Request に Response を返す（サーバー側）
    pub async fn send_response<T: Encodable<C>>(&self, request_id: u64, method: &str, payload: &T) -> Result<(), NetworkError>;

    /// Response 以外のメッセージを受信（Event、サーバー側では Request も）
    pub async fn recv(&self) -> Result<ProtocolMessage, NetworkError>;

    /// Raw bytes の送受信（buffa / zstd を通さない）
    pub async fn send_raw(&self, data: &[u8]) -> Result<(), NetworkError>;
    pub async fn recv_raw(&self) -> Result<Vec<u8>, NetworkError>;

    /// チャネルを閉じる
    pub async fn close(&self) -> Result<(), NetworkError>;
}
```

### 内部動作

- `request()`: メッセージIDを振り、`pending` マップに oneshot を登録。recv ループが Response（または Error）を受信すると対応する oneshot に送信。タイムアウトまでに来なければ `NetworkError::Timeout`
- `send_event()`: Event 型メッセージを送信。応答を待たない
- `send_response()`: 受けた Request の `id` を引き継いだ Response を送信。client の `request()` はこれで解決する
- `recv()`: recv ループが Response 以外（Event / Request、対応する request の無い Error）を `event_rx` に流す。アプリケーションはここから読み取る

---

## 4. エラーハンドリング

| エラー | 原因 | 対処 |
|--------|------|------|
| `NetworkError::NotConnected` | `connect()` の前に `open_channel()` 等を呼んだ | `connect()` を先に呼ぶ |
| `NetworkError::Quic(...)` | channel の stream を開けない（接続が切れている等） | 接続状態を確認し、必要なら再接続する |
| `NetworkError::Protocol(...)` | サーバーに登録されていない channel を open した（nack で即 reject）／サーバーが Error で応答した（`request()` 中に接続が切れた場合も） | channel 名とサーバー側の `register_channel` を確認／エラー内容を確認 |
| `NetworkError::Codec(...)` | payload を `Req` / `Resp` の型に encode / decode できない | 受け取る型が応答の形と合っているか確認 |
| `NetworkError::Timeout` | `request()` の応答が既定 30 秒以内に来ない | サーバーが `send_response` で応答しているか確認。待ち時間は `with_request_timeout` で変えられる |
| `NetworkError::ChannelEof(_)` | 相手が channel を正常に閉じた（`e.is_normal_close()` が true） | エラーではない。受信ループを抜ける |

`NetworkError::HandlerNotFound` はサーバーの `broadcast()`（datagram）に未登録の channel 名を
渡したときのエラーで、client 側では出ない（`server.rs`）。

---

## 関連ドキュメント

- [spec/02: Unified Channel プロトコル仕様](../spec/02-unified-channel/SPEC.md)
- [spec/01: コアコンセプト](../spec/01-core-concept/SPEC.md)

---

**最終更新**: 2026-11-04（club-unison 2.2.1）
