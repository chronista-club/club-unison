# spec/02: Unison Protocol - Unified Channel プロトコル仕様

**バージョン**: 2.0.0
**最終更新**: 2026-05-15
**ステータス**: Stable (v0.9.0 で確定)

---

## 目次

1. [概要](#1-概要)
2. [設計思想](#2-設計思想)
3. [コアメッセージ型](#3-コアメッセージ型)
4. [KDL スキーマ定義言語](#4-kdl-スキーマ定義言語)
5. [メッセージフロー](#5-メッセージフロー)
6. [コード生成](#6-コード生成)
7. [セキュリティ](#7-セキュリティ)
8. [パフォーマンス](#8-パフォーマンス)
9. [バージョニングと互換性](#9-バージョニングと互換性)
10. [今後の拡張](#10-今後の拡張)
11. [関連ドキュメント](#11-関連ドキュメント)

---

## 1. 概要

Unison Protocol の通信層は、**Unified Channel** アーキテクチャに基づく。全ての通信は**チャネル**を通じて行われ、各チャネルは `request`（応答を期待する問い合わせ）と `event`（一方向プッシュ）の2つのメッセージパターンをサポートする。

従来の RPC（`service` / `method`）とストリームチャネル（`channel` / `send` / `recv`）の二重構造を廃止し、チャネル一本に統一することで、プロトコルの複雑さを大幅に削減する。

### 1.1 旧アーキテクチャからの変更点

| 項目 | 旧（v1.x） | 新（v2.0） |
|------|-----------|-----------|
| 通信パターン | RPC (`service`/`method`) + Channel (`send`/`recv`) | **Channel のみ** (`request`/`event`) |
| MessageType | 10 バリアント | **4 バリアント** (Request/Response/Event/Error) |
| ハンドラー登録 | `call_handlers` + `stream_handlers` + `channel_handlers` | **`channel_handlers` のみ** |
| 型生成 | Service trait + `QuicBackedChannel<S,R>` | **`UnisonChannel` のみ** |

---

## 2. 設計思想

### 2.1 目標

- **型安全性**: コンパイル時・実行時の型チェック保証
- **開発者体験**: シンプルで直感的な API
- **多言語サポート**: Rust、TypeScript 等への自動コード生成
- **リアルタイム通信**: 低レイテンシー双方向通信
- **拡張性**: 新しいチャネル、メッセージ型の簡単な追加

### 2.2 設計原則

- **スキーマファースト**: KDL プロトコル定義駆動開発
- **非同期優先**: async/await パターンを基盤
- **チャネル統一**: 全通信パターンをチャネルで表現
- **エラー耐性**: 包括的なエラーハンドリングと回復メカニズム
- **トランスポート非依存**: QUIC、WebSocket、TCP 等に対応

---

## 3. コアメッセージ型

### 3.1 MessageType

全てのメッセージは 4 つの型に分類される。

```rust
pub enum MessageType {
    Request,    // 応答を期待するメッセージ（メッセージIDで紐付け）
    Response,   // Request に対する応答
    Event,      // 一方向プッシュ（応答不要）
    Error,      // エラー
}
```

```mermaid
graph LR
    subgraph "Request/Response パターン"
        REQ["Request<br/>id=42, method='Query'"] --> RES["Response<br/>id=43, response_to=42"]
    end

    subgraph "Event パターン"
        EVT["Event<br/>id=0, method='MemoryEvent'"]
    end

    subgraph "Error パターン"
        ERR["Error<br/>id=44, response_to=42"]
    end
```

### 3.2 ProtocolMessage

全てのプロトコル通信における標準メッセージ形式:

```rust
pub struct ProtocolMessage {
    pub id: u64,               // メッセージID（Requestは一意、Eventは0可）
    pub method: String,        // メソッド名（例: "Query", "MemoryEvent"）
    pub msg_type: MessageType, // メッセージ種別
    pub payload: String,       // JSON 形式のペイロード
}
```

### 3.3 Request/Response 相関

メッセージの相関は `id` フィールドで行う。

| 送信側 | id | response_to | 意味 |
|--------|-----|-------------|------|
| Request | > 0 | 0 | 応答を期待するリクエスト |
| Response | > 0 | > 0 | リクエストに対する応答 |
| Event | 0 | 0 | 一方向メッセージ（応答不要） |
| Error | > 0 | > 0 | リクエストに対するエラー応答 |

`response_to` フィールドは `UnisonPacketHeader` の一部であり、Response/Error は元の Request の `message_id` を `response_to` に設定する。

---

## 4. KDL スキーマ定義言語

### 4.1 基本型

| 型 | 説明 | Rust マッピング | TypeScript マッピング |
|------|-------------|--------------|---------------------|
| `string` | UTF-8 テキスト | `String` | `string` |
| `number` | 数値 | `f64` | `number` |
| `int` | 整数 | `i64` | `number` |
| `bool` | 真偽値 | `bool` | `boolean` |
| `timestamp` | ISO-8601 日時 | `DateTime<Utc>` | `string` |
| `json` | 任意の JSON | `serde_json::Value` | `any` |
| `array` | アイテムのリスト | `Vec<T>` | `T[]` |

### 4.2 フィールド修飾子

- `required=#true`: フィールドが必須（デフォルト: false）
- `default=value`: オプションフィールドのデフォルト値
- `description="text"`: フィールドドキュメンテーション

### 4.3 プロトコル構造

```
Protocol（プロトコル）
├── Metadata（メタデータ） (name, version, namespace, description)
├── Messages（メッセージ） (構造化データ定義)
└── Channels（チャネル）
    ├── request（リクエスト/レスポンス）
    │   └── returns（レスポンス型）
    └── event（一方向イベント）
```

### 4.4 Channel 定義構文

#### 新構文: `request` / `event`

```kdl
channel "<name>" from="<direction>" lifetime="<lifetime>" {
    // Request/Response パターン
    request "<RequestName>" {
        field "<name>" type="<type>" [required=#true]

        returns "<ResponseName>" {
            field "<name>" type="<type>"
        }
    }

    // 一方向イベント
    event "<EventName>" {
        field "<name>" type="<type>" [required=#true]
    }
}
```

#### 属性

| 属性 | 値 | 説明 |
|------|-----|------|
| `from` | `"client"` | クライアントが送信を開始する |
| `from` | `"server"` | サーバーが送信を開始する |
| `from` | `"either"` | 双方が送信可能 |
| `lifetime` | `"persistent"` | 接続中ずっと維持される |
| `lifetime` | `"transient"` | リクエスト単位で開閉される |

#### メッセージブロック

| ブロック | 説明 |
|---------|------|
| `request` | Request/Response パターン。応答を期待するメッセージ |
| `returns` | `request` 内にネストし、レスポンス型を定義 |
| `event` | 一方向プッシュメッセージ。応答不要 |

#### スキーマ例

```kdl
protocol "creo-sync" version="2.0.0" {
    namespace "club.chronista.sync"

    // Query チャネル: Request/Response + Event
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

    // Events チャネル: イベント配信のみ
    channel "events" from="server" lifetime="persistent" {
        event "MemoryEvent" {
            field "event_type" type="string" required=#true
            field "memory_id" type="string" required=#true
            field "category" type="string"
            field "from" type="string"
            field "timestamp" type="string"
        }
    }
}
```

### 4.5 旧構文との互換性（非推奨）

旧 `send`/`recv`/`error` 構文は後方互換として認識されるが、新規スキーマでは非推奨とする。

| 旧構文 | 新構文への変換 |
|--------|-------------|
| `send` + `recv` | `request` + `returns` |
| `send` のみ | `event` |
| `error` | `event`（エラー型として） |

---

## 5. メッセージフロー

### 5.1 チャネル確立

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server

    C->>S: QUIC 接続確立
    S->>C: ServerIdentity 送信（利用可能チャネル一覧）

    C->>S: open_bi() + __channel:query
    Note over S: channel_handlers から<br/>"query" ハンドラーを取得
    Note over C,S: チャネル確立完了
```

### 5.2 Request/Response フロー

チャネル内での Request/Response は、メッセージ ID で紐付けられる。

```mermaid
sequenceDiagram
    participant C as Client (UnisonChannel)
    participant S as Server (ChannelHandler)

    Note over C: request() を呼び出し
    C->>C: メッセージID生成 (id=42)
    C->>C: pending マップに oneshot::Sender を登録
    C->>S: ProtocolMessage {<br/>  id: 42,<br/>  method: "Query",<br/>  msg_type: Request,<br/>  payload: {...}<br/>}

    S->>S: リクエスト処理

    S->>C: ProtocolMessage {<br/>  id: 43,<br/>  method: "Query",<br/>  msg_type: Response,<br/>  payload: {data: ...}<br/>}

    Note over C: recv ループが Response を受信
    C->>C: response_to=42 の pending を解決
    C->>C: oneshot::Sender で呼び出し元に返却
```

### 5.3 Event フロー

Event は一方向プッシュであり、応答を期待しない。

```mermaid
sequenceDiagram
    participant S as Server (ChannelHandler)
    participant C as Client (UnisonChannel)

    S->>C: ProtocolMessage {<br/>  id: 0,<br/>  method: "MemoryEvent",<br/>  msg_type: Event,<br/>  payload: {...}<br/>}

    Note over C: recv ループが Event を受信
    C->>C: event_rx チャネルに送信
    Note over C: recv() で取得可能
```

### 5.4 エラーハンドリング

#### チャネルレベルエラー

| エラー | 原因 | 処理 |
|--------|------|------|
| `HandlerNotFound` | 未登録チャネル名 | Error メッセージを返却 |
| `Protocol` | 不正なメッセージ形式 | Error メッセージを返却 |
| `Timeout` | 応答タイムアウト | pending を Error で解決 |
| `Connection` | QUIC 接続断 | 全 pending を Error で解決 |

#### Request エラー応答

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server

    C->>S: Request (id=42)
    S->>S: 処理失敗

    S->>C: ProtocolMessage {<br/>  id: 43,<br/>  method: "Query",<br/>  msg_type: Error,<br/>  payload: {code: "NOT_FOUND", message: "..."}<br/>}

    Note over C: pending id=42 を Error で解決
```

---

## 6. コード生成

### 6.1 Rust コード生成

KDL スキーマから以下の Rust コードが生成される:

#### メッセージ構造体

`request` と `event` の各フィールドから Serde 注釈付き構造体を生成。

```rust
// request "Query" から生成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

// returns "Result" から生成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// event "QueryError" から生成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryError {
    pub code: String,
    pub message: String,
}

// event "MemoryEvent" から生成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub event_type: String,
    pub memory_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}
```

#### Connection 構造体

プロトコル内の全チャネルを `UnisonChannel` としてまとめた接続構造体を生成。

```rust
/// コード生成で自動生成される接続構造体
pub struct CreoSyncConnection {
    pub query: UnisonChannel,
    pub events: UnisonChannel,
    pub control: UnisonChannel,
    pub messaging: UnisonChannel,
    pub urgent: UnisonChannel,
}

/// ConnectionBuilder トレイト
#[async_trait]
pub trait CreoSyncConnectionBuilder {
    async fn build(client: &ProtocolClient) -> Result<CreoSyncConnection, NetworkError>;
}
```

### 6.2 TypeScript コード生成（計画中）

- **インターフェース定義**: 全型の TypeScript インターフェース
- **クライアントクラス**: Promise ベースのクライアント実装
- **型ガード**: 実行時型検証

---

## 7. セキュリティ

### 7.1 認証と認可

- プロトコルレベルの認証は未指定（トランスポートレイヤーの責任）
- チャネルレベルの認可はハンドラー実装で対応
- セッション管理はアプリケーション固有トークンで実現

### 7.2 入力検証

- 必須フィールドの自動検証
- 全パラメータの型チェック
- カスタム検証はハンドラー実装で対応

### 7.3 トランスポートセキュリティ

- 本番使用には TLS（QUIC、WSS）を推奨
- 証明書検証とピン留め
- 接続暗号化と完全性

v0.7.0 以降、 TLS の cert / trust 戦略は **明示選択 API** (`CertSource` / `TrustAnchors`) で表現する。 v0.8.0 で **Builder API** (`QuicServer::builder()` / `QuicClient::builder()`) が推奨形となり、 v0.9.0 で旧 `configure_server()` / `configure_client()` の暗黙 default は削除された。 詳細は [`crate::network::cert`](../../crates/unison-protocol/src/network/cert.rs) / [`crate::network::trust`](../../crates/unison-protocol/src/network/trust.rs) と [`examples/builder_api.rs`](../../crates/unison-protocol/examples/builder_api.rs) 参照。

---

## 8. パフォーマンス

### 8.1 メッセージサイズ

- JSON ベースのシリアライゼーション
- 典型的なメッセージオーバーヘッド: 100-200 バイト
- 2KB 以上のペイロードは zstd で自動圧縮

### 8.2 レイテンシー

- サブミリ秒のプロトコルオーバーヘッド
- Request/Response はメッセージ ID ベースの即座の相関
- チャネル内 HoL Blocking は許容（シンプルさ優先）

### 8.3 スループット

- チャネル間の独立性により並行処理を最大化
- 非同期ランタイム (tokio) を通じた同時リクエストハンドリング

### 8.4 Wire format (v0.9.0+ pluggable hook、 具体実装は v0.10+)

v0.9.0 で `crate::wire::WireFormat` trait による wire format 抽象化を導入した。
現時点の default wire format は [`crate::packet`] 経由の **rkyv archive** (zero-copy)。
v0.10+ で以下の 3 実装が並ぶ予定:

| 実装 | format | 用途 |
|------|--------|------|
| `RkyvWire` | rkyv archive | Rust 内 zero-copy hot path |
| `BuffaWire` | Protocol Buffers (Anthropic 製 [`buffa`](https://crates.io/crates/buffa)) | polyglot, schema evolution |
| `MessagePackWire` | MessagePack ([`zerompk`](https://crates.io/crates/zerompk) 等) | polyglot, コンパクト |

設計詳細は [`design/wire-format.md`](../../design/wire-format.md) 参照。

### 8.5 Datagram (QUIC unreliable / unordered、 ≤MTU)

v0.9.0 で `QuicClient::send_datagram` / `recv_datagram` MVP API を新設した。 これは
**connection-level thin wrapper** (= channel 抽象を経由しない、 caller が demux
header を payload に含める責任)。 想定用途:

- **3DCG position+rotation transform 大量配信** (= 60Hz / 120Hz、 1 frame で 100-1000
  peer broadcast、 unreliable OK で latency 優先)
- **low-latency event push** (= ack 不要 fire-and-forget)
- **heartbeat / presence**

サイズは **MTU 安全値 ≤1300B** (= IP MTU 1500 - IP/UDP/QUIC header)。 超過すると
`SendDatagramError::TooLarge` が返り fragment 不可。

#### Unified Channel narrative との整合

datagram は性質上 **Request/Response パターンに乗らない** (= ack なし)、 spec/02 §3.1
の MessageType のうち **Event のみ** 適合する。 v0.10+ で `event "X" backend="datagram"`
KDL schema 拡張と一緒に channel API へ統合予定:

```kdl
channel "position" from="server" lifetime="persistent" {
    event "Transform" backend="datagram" {
        field "id" type="string"
        field "pos" type="json"   // [x, y, z]
        field "rot" type="json"   // [x, y, z, w]
    }
}
```

それまでは connection-level API (= `QuicClient::send_datagram`) で raw bytes 直送、
caller が channel ID 等の demux header を payload に含める。

#### HoL blocking (§8.2 の補足)

§8.2 の「チャネル内 HoL Blocking 許容」 は **stream channel** 前提。 datagram は HoL
blocking なし (= UDP-like)、 v0.10+ で channel API 統合時に「stream channel = HoL 許容、
datagram channel = HoL なし」 と仕様で明示する。

#### benchmark baseline

`benches/datagram.rs` で `payload {64, 1300} × burst {100, 1000}` の 4 ケース計測。
`benches/RESULTS.md` 参照。

---

## 9. バージョニングと互換性

### 9.1 プロトコルバージョニング

- セマンティックバージョニング（MAJOR.MINOR.PATCH）
- v2.0.0: Unified Channel への移行（破壊的変更）

### 9.2 後方互換性

- 旧 `send`/`recv` KDL 構文はパーサーが認識し、内部で `request`/`event` に変換
- 旧 `service`/`method` 構文は非推奨警告を出力
- 新しいオプションフィールドの追加: 互換
- 新しい `request`/`event` の追加: 互換

### 9.3 前方互換性

- デシリアライゼーション時に不明フィールドは無視
- 不明メソッドは `Error` メッセージで応答
- バージョン不整合ハンドリング

---

## 10. 今後の拡張

### 10.1 計画中の機能

- **スキーマ進化**: 実行時スキーマ更新とマイグレーション
- **バッチ操作**: 単一チャネルでの複数リクエスト並行実行
- **チャネルメトリクス**: スループット、レイテンシー、エラー率の自動計測

### 10.2 言語サポート拡張

- TypeScript クライアント・サーバー生成の完成
- Python、Go 等への展開

---

## 11. 関連ドキュメント

### 仕様書

- [spec/01: コアコンセプト](../01-core-concept/SPEC.md) - トランスポート層（QUIC）
- [spec/03: チャネル仕様](../03-stream-channels/SPEC.md) - UnisonChannel 仕様

### 設計ドキュメント

- [KDL スキーマ例](../../schemas/) - 実際のスキーマ定義

### 参考資料

- [KDL 仕様](https://kdl.dev/)
- [JSON スキーマ](https://json-schema.org/)

---

**仕様バージョン**: 2.0.0-draft
**最終更新**: 2026-02-16
**ステータス**: Draft
