# Unison Protocol アーキテクチャ設計

**バージョン**: 0.2.0-draft
**最終更新**: 2026-02-16
**ステータス**: Draft

---

## 目次

1. [概要](#1-概要)
2. [ワークスペース構成](#2-ワークスペース構成)
3. [unison-protocol モジュール構成](#3-unison-protocol-モジュール構成)
4. [データフロー](#4-データフロー)
5. [エラーハンドリング](#5-エラーハンドリング)
6. [拡張ポイント](#6-拡張ポイント)

---

## 1. 概要

Unison ProtocolはCargoワークスペースとして構成され、プロトコル定義・パーサー・ネットワーク通信を一つのクレートに統合している。QUICベースのChannel指向通信を提供する。KDLスキーマからの型コード生成は `club-kdl-codegen` crate に分離されている。

---

## 2. ワークスペース構成

```
unison/
  Cargo.toml              -- ワークスペースルート (edition = 2024, rust-version = 1.93)
  schemas/                 -- KDLプロトコル定義（テスト用）
  crates/
    unison-protocol/       -- コアクレート（パーサー、ネットワーク）
    unison-agent/          -- エージェント実装
```

### ワークスペース共通設定

| 設定 | 値 |
|------|-----|
| edition | 2024 |
| rust-version | 1.93 |
| version | 0.3.1 |
| resolver | 2 |

### 主要依存クレート

| 用途 | クレート |
|------|---------|
| シリアライゼーション | serde, serde_json, rkyv |
| QUIC | quinn 0.11, rustls 0.23 |
| 圧縮 | zstd |
| 非同期ランタイム | tokio |
| KDLパース | kdl, club-kdl |
| CGP | cgp 0.4.2 |
| エラーハンドリング | thiserror, anyhow, miette |

---

## 3. unison-protocol モジュール構成

### 3.1 トップレベルモジュール

```
crates/unison-protocol/src/
  lib.rs                   -- エントリポイント、UnisonProtocol構造体
  prelude.rs               -- よく使用される型のreexport
  core/
    mod.rs                 -- コア型定義
  parser/
    mod.rs                 -- SchemaParserエントリポイント
    schema.rs              -- ParsedSchema、スキーマ構造
    types.rs               -- TypeRegistry、型定義
  packet/
    mod.rs                 -- UnisonPacket、UnisonPacketBuilder、UnisonPacketView
    header.rs              -- UnisonPacketHeader (56 bytes)、PacketType
    flags.rs               -- PacketFlags ビットフラグ
    payload.rs             -- Payloadable trait、各種ペイロード型
    config.rs              -- PacketConfig、CompressionConfig
    serialization.rs       -- PacketSerializer / PacketDeserializer
  context/
    mod.rs                 -- CGPベースコンテキスト
    adapter.rs             -- コンテキストアダプター
    handlers.rs            -- ハンドラー実装
  network/
    mod.rs                 -- NetworkError、ProtocolMessage、MessageType、トレイト群
    quic.rs                -- QuicClient、QuicServer、UnisonStream
    server.rs              -- ProtocolServer（チャネルハンドラー管理）
    client.rs              -- ProtocolClient（チャネル開設・通信）
    channel.rs             -- UnisonChannel、StreamSender/Receiver
    identity.rs            -- ServerIdentity、ChannelInfo、ChannelUpdate
    context.rs             -- ConnectionContext（接続状態管理）
    service.rs             -- Service trait、UnisonService、RealtimeService
```

### 3.2 network/ 配下の責務

```mermaid
graph TB
    subgraph "network/ モジュール"
        MOD["mod.rs<br/>-- NetworkError enum<br/>-- ProtocolMessage struct<br/>-- MessageType enum<br/>-- トレイト定義<br/>(UnisonClient, UnisonServer,<br/>SystemStream 等)"]

        QUIC["quic.rs<br/>-- QuicClient: QUIC接続・送受信<br/>-- QuicServer: 接続受付・ルーティング<br/>-- UnisonStream: 双方向ストリーム実装<br/>-- read_frame / write_frame<br/>-- TLS証明書管理"]

        SERVER["server.rs<br/>-- ProtocolServer: チャネルハンドラーレジストリ<br/>-- ChannelHandler 登録・ルーティング<br/>-- ServerIdentity 構築<br/>-- UnisonService 管理"]

        CLIENT["client.rs<br/>-- ProtocolClient: チャネル通信<br/>-- open_channel(): UnisonChannel開設<br/>-- Identity受信処理<br/>-- ConnectionContext管理"]

        CHANNEL["channel.rs<br/>-- UnisonChannel: 統合チャネル型<br/>(request/response + event push)"]

        IDENTITY["identity.rs<br/>-- ServerIdentity: ノード自己紹介<br/>-- ChannelInfo / ChannelDirection<br/>-- ChannelStatus / ChannelUpdate<br/>-- __identity メッセージ変換"]

        CONTEXT["context.rs<br/>-- ConnectionContext: 接続状態<br/>-- ChannelHandle: チャネルメタデータ<br/>-- Arc&lt;RwLock&gt; による並行安全性"]

        SERVICE["service.rs<br/>-- Service trait: 高レベルサービスIF<br/>-- RealtimeService trait<br/>-- UnisonService: Service実装<br/>-- ServiceConfig / ServiceStats"]
    end

    MOD --> QUIC
    MOD --> SERVER
    MOD --> CLIENT
    MOD --> CHANNEL
    MOD --> IDENTITY
    MOD --> CONTEXT
    MOD --> SERVICE

    QUIC --> SERVER
    CLIENT --> QUIC
    CLIENT --> CHANNEL
    CLIENT --> CONTEXT
    CLIENT --> IDENTITY
    SERVER --> IDENTITY
    SERVER --> CONTEXT
    CHANNEL --> QUIC
    SERVICE --> QUIC
```

### 3.3 packet/ 配下の責務

| ファイル | 責務 |
|---------|------|
| `mod.rs` | `UnisonPacket<T>` -- ジェネリックフレーム構造体。`Bytes`で生データ保持、遅延デシリアライズ |
| `header.rs` | `UnisonPacketHeader` -- 56バイト固定長ヘッダー。version, packet_type, flags, lengths, IDs |
| `flags.rs` | `PacketFlags` -- ビットフラグ（COMPRESSED, PRIORITY_HIGH, REQUIRES_ACK等） |
| `payload.rs` | `Payloadable` trait + ペイロード型: `StringPayload`, `BytesPayload`, `JsonPayload`, `RkyvPayload<T>`, `EmptyPayload` |
| `config.rs` | `PacketConfig` -- 最大ペイロードサイズ、圧縮設定 |
| `serialization.rs` | `PacketSerializer` / `PacketDeserializer` -- rkyv + zstd のシリアライズ/デシリアライズ |

---

## 4. データフロー

### 4.1 Request/Response フロー（UnisonChannel経由）

```mermaid
sequenceDiagram
    participant App as Application
    participant UC as UnisonChannel
    participant US as UnisonStream
    participant Net as QUIC Connection
    participant QS as QuicServer
    participant CH as ChannelHandler

    App->>UC: request("method", payload)
    UC->>UC: ProtocolMessage作成<br/>(id, method, Request, payload)
    UC->>US: send(message)
    US->>US: message.into_frame()
    US->>Net: write_frame(frame_bytes)

    Net->>QS: accept_bi()
    QS->>QS: read_frame() -> ProtocolMessage
    QS->>QS: method.strip_prefix("__channel:")
    QS->>CH: handler(ctx, UnisonStream)
    Note over CH: チャネルハンドラー内で<br/>request を処理し response を返す

    CH-->>Net: write_frame(response)
    Net-->>US: read_frame()
    US-->>UC: pending requestのoneshotに送信
    UC-->>App: Result<Value>
```

### 4.2 Channelフロー（開設〜通信）

```mermaid
sequenceDiagram
    participant App as Application
    participant PC as ProtocolClient
    participant Net as QUIC Connection
    participant QS as QuicServer
    participant PS as ProtocolServer
    participant CH as ChannelHandler

    App->>PC: open_channel("events")

    PC->>Net: open_bi()
    PC->>PC: ProtocolMessage作成<br/>(method: "__channel:events",<br/>type: Request)
    PC->>PC: write_frame(frame_bytes)
    PC->>PC: UnisonStream::from_streams()
    PC->>PC: UnisonChannel::new(stream)
    PC-->>App: UnisonChannel

    Net->>QS: accept_bi()
    QS->>QS: read_frame() -> ProtocolMessage
    QS->>QS: method.strip_prefix("__channel:")
    QS->>PS: get_channel_handler("events")
    PS-->>QS: ChannelHandler

    QS->>CH: handler(ctx, UnisonStream)
    Note over CH: ストリームは生存したまま<br/>ChannelHandlerが管理

    loop チャネル通信
        App->>PC: channel.request("method", payload)
        PC->>Net: write_frame(Request)
        Net->>CH: read_frame() -> Request
        CH-->>Net: write_frame(Response)
        Net-->>PC: recv loop -> pending に振り分け
        PC-->>App: Result<Value>
    end

    App->>PC: channel.close()
    PC->>Net: send_stream.finish()
```

### 4.3 Identityフロー

```mermaid
sequenceDiagram
    participant C as ProtocolClient
    participant Net as QUIC Connection
    participant S as QuicServer
    participant PS as ProtocolServer
    participant CTX as ConnectionContext

    C->>Net: QUIC接続確立
    Net->>S: accept(connecting)

    S->>CTX: ConnectionContext::new()
    S->>PS: build_identity()
    PS->>PS: channel_handlers.keys() から<br/>ChannelInfo一覧を構築
    PS-->>S: ServerIdentity

    S->>CTX: set_identity(identity)
    S->>S: identity.to_protocol_message()<br/>(__identity, Event)
    S->>Net: open_bi() + write_all(frame) + finish()

    Net-->>C: transport.receive()
    C->>C: response.method == "__identity"
    C->>C: ServerIdentity::from_protocol_message()
    C->>CTX: context.set_identity(identity)

    Note over C: server_identity() で<br/>利用可能チャネル一覧にアクセス可能
```

---

## 5. エラーハンドリング

### 5.1 NetworkError enum

`NetworkError` はネットワーク層の全エラーを統一的に表現する。

```rust
pub enum NetworkError {
    Connection(String),         // 接続エラー（切断、タイムアウト等）
    Protocol(String),           // プロトコルレベルのエラー（不正メッセージ等）
    Serialization(serde_json::Error),  // JSONシリアライゼーションエラー
    FrameSerialization(SerializationError), // rkyv/zstdフレームエラー
    Quic(String),               // QUICトランスポートエラー
    Timeout,                    // タイムアウト
    HandlerNotFound { method: String }, // 未登録メソッド呼び出し
    NotConnected,               // 未接続状態でのオペレーション
    UnsupportedTransport(String), // 非サポートトランスポート
}
```

### 5.2 エラー発生箇所

| エラー種別 | 発生箇所 | 原因 |
|-----------|---------|------|
| `Connection` | QuicClient, UnisonStream | 接続断、ストリーム非アクティブ |
| `Protocol` | ProtocolClient, QuicServer | メッセージパースエラー、不正な応答 |
| `Serialization` | ProtocolMessage | JSONシリアライゼーション/デシリアライゼーション |
| `FrameSerialization` | UnisonPacket | rkyv/zstdエラー、バージョン不互換 |
| `Quic` | QuicClient, QuicServer, UnisonStream | QUICストリーム操作エラー |
| `Timeout` | RealtimeService | 受信タイムアウト |
| `HandlerNotFound` | ProtocolServer | 未登録メソッドの呼び出し |
| `NotConnected` | ProtocolClient | 接続前のチャネル/RPC操作 |

---

## 6. 拡張ポイント

### 6.1 Trait一覧

以下のトレイトにより、カスタム実装の差し込みが可能である。

#### クライアント側

| Trait | 責務 | 主要メソッド |
|-------|------|------------|
| `UnisonClient` | 接続管理 | `connect()`, `disconnect()`, `is_connected()` |

> **Note**: 旧 `ProtocolClientTrait`, `UnisonClientExt` は Unified Channel 統合により削除済み。

#### サーバー側

| Trait | 責務 | 主要メソッド |
|-------|------|------------|
| `UnisonServer` | サーバーライフサイクル | `listen()`, `stop()`, `is_running()` |

> **Note**: 旧 `ProtocolServerTrait`, `UnisonServerExt` は Unified Channel 統合により削除済み。ハンドラー登録は `ProtocolServer::register_channel()` メソッドで行う。

#### チャネル・ストリーム・サービス

| Trait / 型 | 責務 | 主要メソッド |
|------------|------|------------|
| `UnisonChannel` | 統合チャネル（request/response + event） | `request()`, `send_event()`, `recv()`, `close()` |
| `SystemStream` | 双方向ストリームI/O | `send()`, `receive()`, `is_active()`, `close()`, `get_handle()` |
| `Service` | 高レベルサービスIF | `service_type()`, `service_name()`, `handle_request()`, `shutdown()` |
| `RealtimeService` | リアルタイム通信拡張 | `send_realtime()`, `receive_with_timeout()`, `get_performance_stats()` |

### 6.2 拡張パターン

```mermaid
graph TB
    subgraph "アプリケーション層"
        APP["アプリケーション"]
    end

    subgraph "チャネル層"
        UCH["UnisonChannel<br/>(request + event 統合)"]
    end

    subgraph "拡張ポイント"
        UC["UnisonClient"]
        US["UnisonServer"]
        SS["SystemStream"]
        SVC["Service / RealtimeService"]
    end

    subgraph "デフォルト実装"
        PC["ProtocolClient"]
        PS["ProtocolServer"]
        USTREAM["UnisonStream"]
        USVC["UnisonService"]
    end

    APP --> UCH
    APP --> SVC

    UCH --> PC
    UC --> PC
    US --> PS
    SS --> USTREAM
    SVC --> USVC
    SS --> SVC
```

カスタム実装の例:
- `SystemStream` を実装して、QUIC以外のトランスポート上でストリームを動作させる
- `Service` を実装して、ドメイン固有のサービスロジックを提供する
- `UnisonChannel` をベースに、チャネルハンドラーでドメイン固有のプロトコルを構築する

---

**設計バージョン**: 0.2.0-draft
**最終更新**: 2026-02-16
**ステータス**: Draft
