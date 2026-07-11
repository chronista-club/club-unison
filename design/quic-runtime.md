# QUIC Runtime 統合

## 1. 概要

本ドキュメントでは、Unison ProtocolにおけるQUIC Runtime統合の設計を記述する。QUICトランスポート層は以下の要素で構成される:

- **ALPN**: raw QUIC は ALPN `"unison"` を negotiate（`network::UNISON_ALPN`）
- **ConnectionContext**: 接続ごとの状態管理（identity、チャネル追跡）
- **Identity Handshake**: 接続直後のサーバー自己紹介プロトコル
- **チャネルルーティング**: `__channel:` プレフィックスによるチャネル多重化
- **Length-Prefixed Framing**: 4バイトBE長プレフィックスによるフレーム境界
- **UnisonChannel**: 統合チャネル型（request/response + event push）

### ALPN（v1.3.0+）

QUIC は仕様（RFC 9001 §8.1）で ALPN を必須とする。raw QUIC の server
（`quic.rs`）と client（`trust.rs`）は SSOT である `network::UNISON_ALPN`
（= `"unison"`）を negotiate する。空 ALPN は仕様逸脱であり、Apple
`NWProtocolQUIC` 等の厳格実装（Swift client）と interop できないため v1.3.0 で
明示設定した。

> ⚠️ **互換性（重要）**: ALPN を設定した server は、ALPN を送らない**旧 client を
> `no_application_protocol` で拒否する**。QUIC は ALPN 必須なので、rustls/quinn は
> plain TLS（client が ALPN を出さなければ enforce しない）と違い handshake 時に
> **enforce する**（empty-ALPN client での実測で確認済み）。したがって v1.3.0 は
> raw QUIC client にとって **breaking** で、**後方互換ではない**:
> - **Rust client / Swift client**: `"unison"` を送るため OK。
> - **Ruby client（FFI）**: gem が pin する `club-unison` を **1.3.0+ に上げて
>   ビルドし直す必要がある**（旧 lock の 1.0.x は empty ALPN → 拒否される）。
> - **WebTransport（TS）**: HTTP/3 の `"h3"` 別 ingress なので**無影響**。
>
> WebTransport 経路は HTTP/3 上のため ALPN は `"h3"` 固定（`wtransport` 依存が内部
> 設定）で、本 const は適用されない（別 ingress）。回帰テスト:
> `crates/unison-protocol/tests/test_alpn_enforcement.rs`。

全体のアーキテクチャ層における位置付け:

```
Application Layer
    |
Service Layer   --- UnisonChannel (統合チャネル: request/response + event)
    |
Protocol Layer  --- ProtocolMessage / ProtocolFrame
    |
Transport Layer --- UnisonStream / read_frame / write_frame
    |
Network Layer   --- quinn::Connection (QUIC/TLS 1.3)
```

## 2. ConnectionContext

**ファイル**: `crates/unison-protocol/src/network/context.rs`

ConnectionContextは、QUIC接続ごとの状態を管理する構造体である。各接続に対して一意のUUID、Identity情報、アクティブチャネルの追跡を提供する。

### 構造体定義

```rust
pub struct ConnectionContext {
    /// 接続の一意識別子
    pub connection_id: Uuid,
    /// サーバーから受信したIdentity情報
    identity: Arc<RwLock<Option<ServerIdentity>>>,
    /// アクティブなチャネルのマップ（チャネル名 -> ハンドル）
    channels: Arc<RwLock<HashMap<String, ChannelHandle>>>,
}

pub struct ChannelHandle {
    pub channel_name: String,
    pub stream_id: u64,
    pub direction: ChannelDirection,
}

pub enum ChannelDirection {
    Send,
    Receive,
    Bidirectional,
}
```

### ライフサイクル

```mermaid
stateDiagram-v2
    [*] --> Created: ConnectionContext::new()
    Created --> IdentityReceived: set_identity()
    IdentityReceived --> Active: register_channel()
    Active --> Active: register_channel() / remove_channel()
    Active --> Closing: 接続クローズ
    IdentityReceived --> Closing: 接続クローズ
    Closing --> [*]
```

### 設計判断

- **Arc\<RwLock\<\>\> による並行保護**: 1つのQUIC接続に対して複数のストリームハンドラが並行動作するため、identity と channels を個別にロック管理する。これにより、チャネル登録がidentity参照をブロックしない。
- **UUID による接続識別**: `Uuid::new_v4()` で生成される128ビットの一意識別子。接続ログやデバッグ時のトレーサビリティに使用する。

## 3. Identity Handshake

**ファイル**: `crates/unison-protocol/src/network/identity.rs`

QUICサーバーは接続を受け入れた直後に、サーバー自身の情報をクライアントに通知する。これにより、クライアントは接続先の能力（提供チャネル一覧、バージョン）を動的に把握できる。

### ServerIdentity 構造体

```rust
pub struct ServerIdentity {
    pub name: String,           // サーバー名（例: "unison"）
    pub version: String,        // バージョン（例: "0.1.0"）
    pub namespace: String,      // ネームスペース（例: "default"）
    pub channels: Vec<ChannelInfo>, // 利用可能なチャネル一覧
    pub metadata: serde_json::Value, // 拡張メタデータ
}

pub struct ChannelInfo {
    pub name: String,
    pub direction: ChannelDirection,  // ServerToClient / ClientToServer / Bidirectional
    pub lifetime: String,             // "persistent" / "transient"
    pub status: ChannelStatus,        // Available / Busy / Unavailable
}
```

### シーケンス図

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server

    C->>S: QUIC接続確立 (TLS 1.3 Handshake)
    activate S

    Note over S: ConnectionContext::new()
    Note over S: server.build_identity()
    Note over S: ctx.set_identity(identity)

    S->>C: open_bi() -> ServerIdentity送信
    Note right of S: method: "__identity"<br/>msg_type: Event<br/>payload: ServerIdentity JSON

    deactivate S

    Note over C: ServerIdentity受信<br/>チャネル一覧を把握

    loop ストリーム受付ループ
        C->>S: accept_bi() -> リクエスト
        S->>C: レスポンス
    end
```

### build_identity の動作

`ProtocolServer::build_identity()` は、登録済みのチャネルハンドラから動的に `ServerIdentity` を構築する:

```rust
pub async fn build_identity(&self) -> ServerIdentity {
    let mut identity = ServerIdentity::new(
        &self.server_name,
        &self.server_version,
        &self.server_namespace,
    );
    let handlers = self.channel_handlers.read().await;
    for channel_name in handlers.keys() {
        identity.add_channel(ChannelInfo {
            name: channel_name.clone(),
            direction: ChannelDirection::Bidirectional,
            lifetime: "persistent".to_string(),
            status: ChannelStatus::Available,
        });
    }
    identity
}
```

### ChannelUpdate（リアルタイム通知）

接続中にチャネルの追加・削除・状態変更が発生した場合、`ChannelUpdate` を通じてクライアントに通知する:

```rust
#[serde(tag = "type", content = "channel")]
pub enum ChannelUpdate {
    Added(ChannelInfo),
    Removed(String),
    StatusChanged { name: String, status: ChannelStatus },
}
```

## 4. チャネルルーティング

**ファイル**: `crates/unison-protocol/src/network/quic.rs` (`handle_connection` 関数内)

QUIC接続上では、全通信がチャネルとして処理される。ルーティングは `ProtocolMessage.method` フィールドのプレフィックスで判別する。

### ルーティング規則

| プレフィックス | 種別 | 処理 |
|---|---|---|
| `__channel:{name}` | チャネル | `get_channel_handler` で対応ハンドラに委譲 |
| `__identity` | Identity | ServerIdentity の送信 |

### フロー図

```mermaid
flowchart TD
    A[accept_bi] --> B[read_frame / read_to_end]
    B --> C[ProtocolFrame::from_bytes]
    C --> D[ProtocolMessage::from_frame]
    D --> E{method prefix?}

    E -->|"__channel:{name}"| F[チャネルハンドラ]
    E -->|その他 Request| G["process_message (deprecated)"]

    F --> I[UnisonStream::from_streams]
    I --> J[handler ctx, stream]
    J --> K["UnisonChannel で<br/>request/response + event 処理"]

    G --> L[レスポンス送信 + finish]
```

### 通信モデル

Unified Channel 統合により、全通信はチャネル経由で行われる:

- **チャネル内 Request/Response**: `UnisonChannel::request()` でメッセージIDベースの相関。応答は `pending` マップの oneshot で紐付け
- **チャネル内 Event**: `UnisonChannel::send_event()` で一方向プッシュ。応答不要

## 5. Length-Prefixed Framing

**ファイル**: `crates/unison-protocol/src/network/quic.rs`

QUICストリーム上でのメッセージ境界を明確にするため、Length-Prefixed Framing を採用する。

### read_frame

```rust
pub async fn read_frame(recv: &mut RecvStream) -> Result<bytes::Bytes> {
    // 1. 4バイトのBigEndian長を読む
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    // 2. サイズ上限チェック (MAX_MESSAGE_SIZE = 8MB)
    if len > MAX_MESSAGE_SIZE {
        return Err(anyhow!("Frame too large: {} bytes", len));
    }

    // 3. データ本体を読む
    let mut data = vec![0u8; len];
    recv.read_exact(&mut data).await?;
    Ok(bytes::Bytes::from(data))
}
```

### write_frame

```rust
pub async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    let len = (data.len() as u32).to_be_bytes();
    send.write_all(&len).await?;
    send.write_all(data).await?;
    Ok(())
}
```

### フレームフォーマット

```
+-------------------+-------------------+
| Length (4 bytes)   | Data (N bytes)    |
| Big-Endian u32     | ProtocolFrame     |
+-------------------+-------------------+
```

- **最大サイズ**: `MAX_MESSAGE_SIZE = 8 * 1024 * 1024` (8MB)
- **Data部**: `ProtocolFrame`（= `UnisonPacket<RkyvPayload<ProtocolMessage>>`）のバイト列

### 後方互換: read_to_end フォールバック

サーバー側の `handle_connection` では、`read_frame` が失敗した場合に `read_to_end` にフォールバックする。これにより、Length-Prefixed Framing に対応していない旧クライアントとの互換性を維持する:

```rust
let request_result = match read_frame(&mut recv_stream).await {
    Ok(frame_bytes) => ProtocolFrame::from_bytes(&frame_bytes)
        .and_then(|frame| ProtocolMessage::from_frame(&frame)),
    Err(_) => {
        // read_to_end にフォールバック（旧クライアント互換）
        match recv_stream.read_to_end(MAX_MESSAGE_SIZE).await {
            Ok(data) => {
                let frame_bytes = bytes::Bytes::from(data);
                ProtocolFrame::from_bytes(&frame_bytes)
                    .and_then(|frame| ProtocolMessage::from_frame(&frame))
            }
            Err(e) => { /* エラー処理 */ }
        }
    }
};
```

## 6. UnisonChannel（統合チャネル型）

**ファイル**: `crates/unison-protocol/src/network/channel.rs`

`UnisonChannel` は、全チャネルを統一する型。内部に recv ループを持ち、受信メッセージを Response（pending request の oneshot に送信）と Event（event queue に送信）に振り分ける。

### 構造体定義

```rust
pub struct UnisonChannel {
    stream: Arc<Mutex<UnisonStream>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<ProtocolMessage>>>>,
    event_rx: mpsc::Receiver<ProtocolMessage>,
    recv_task: JoinHandle<()>,
}

impl UnisonChannel {
    /// Request/Response（メッセージIDで紐付け）
    pub async fn request(&self, method: &str, payload: Value) -> Result<Value, NetworkError>;

    /// 一方向送信（応答不要）
    pub async fn send_event(&self, method: &str, payload: Value) -> Result<(), NetworkError>;

    /// イベント受信（サーバーからのプッシュ）
    pub async fn recv(&mut self) -> Result<ProtocolMessage, NetworkError>;

    /// チャネルを閉じる
    pub async fn close(&mut self) -> Result<(), NetworkError>;
}
```

### チャネルの型ファミリー

```mermaid
classDiagram
    class SystemStream {
        <<trait>>
        +send(data: Value)
        +receive() Value
        +is_active() bool
        +close()
        +get_handle() StreamHandle
    }

    class UnisonStream {
        -stream_id: u64
        -method: String
        -connection: Arc~Connection~
        -send_stream: Arc~Mutex~Option~SendStream~~~
        -recv_stream: Arc~Mutex~Option~RecvStream~~~
        -is_active: Arc~AtomicBool~
    }

    class UnisonChannel {
        -stream: Arc~Mutex~UnisonStream~~
        -pending: Arc~Mutex~HashMap~u64, oneshot::Sender~~
        -event_rx: mpsc~Receiver~ProtocolMessage~~
        +request(method, payload) Value
        +send_event(method, payload)
        +recv() ProtocolMessage
        +close()
    }

    class StreamSender~T~ {
        -tx: mpsc~Sender~T~~
        +send(msg: T)
    }

    class StreamReceiver~T~ {
        -rx: mpsc~Receiver~T~~
        +recv() Option~T~
    }

    SystemStream <|.. UnisonStream
    UnisonStream --* UnisonChannel : wraps
    StreamSender --* BidirectionalChannel
    StreamReceiver --* BidirectionalChannel
```

### 本番用 vs テスト用

| 型 | 用途 | トランスポート |
|---|---|---|
| `UnisonChannel` | **本番通信** | QUIC双方向ストリーム |
| `StreamSender<T>` / `StreamReceiver<T>` | テスト、プロセス内通信 | mpsc チャネル |

### request / recv の内部動作

```mermaid
sequenceDiagram
    participant App as アプリケーション
    participant UC as UnisonChannel
    participant US as UnisonStream
    participant QUIC as QUIC Stream

    App->>UC: request("method", payload)
    UC->>UC: ProtocolMessage作成 (id=N, type=Request)
    UC->>UC: pending[N] = oneshot::channel()
    UC->>US: send(message)
    US->>QUIC: write_frame(frame_bytes)

    Note over UC: recv ループが常時稼働
    QUIC-->>UC: read_frame() -> ProtocolMessage
    UC->>UC: msg_type == Response?
    UC->>UC: pending[N].send(response)
    UC-->>App: oneshot.recv() -> Result<Value>

    App->>UC: recv() (イベント受信)
    Note over UC: recv ループが Event を検出
    QUIC-->>UC: read_frame() -> ProtocolMessage
    UC->>UC: msg_type == Event?
    UC->>UC: event_rx.send(message)
    UC-->>App: event_rx.recv() -> ProtocolMessage
```

## 関連ドキュメント

- [アーキテクチャ](./architecture.md) -- システム全体のレイヤー構成
- [パケット設計](./packet.md) -- UnisonPacket / ProtocolFrame のバイナリフォーマット
- [プロトコル仕様](../spec/PROTOCOL_SPEC.md) -- KDLスキーマとメッセージ種別
