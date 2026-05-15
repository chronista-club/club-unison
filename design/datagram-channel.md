# design/datagram-channel.md — Datagram Channel 設計 (v0.10.0)

**バージョン**: 0.1-skeleton (v0.10.0 で Hearing 完了、 実装と並行に詳細化)
**最終更新**: 2026-05-15
**ステータス**: Draft (= skeleton SDG、 詳細は実装中に追記)

---

## 1. 背景

v0.9.0 で QUIC datagram MVP (= `QuicClient::send_datagram` / `recv_datagram`) を導入した
が、 channel narrative の外側にぶら下がる connection-level API で、 caller が demux header
を自前実装する必要があった。

spec/02 §2.1 の core principle 「**通信は全てチャネル経由**」 と整合を取るため、 v0.10.0
で datagram を **KDL channel API に統合** する。 v0.9.0 の connection-level API は
escape hatch として残存させ、 新規 caller は datagram channel API を推奨。

関連 v0.10+ task:

- `mem_1Cb46rfagUw7Qqc9WthSsP` (= QuicServer datagram API、 本 doc の対象)
- `mem_1Cb46rzNn2gC7BPN1fBFJB` (= ProtocolClient connection event hook、 v0.10.0 Step 2)
- `mem_1Cb46sKeeZvZSLmdWFgKVU` (= ClientIdentity 概念検討、 v0.10.0 Step 3)

---

## 2. Mental model

### 2.1 1 channel = 1 (virtual) stream

| | Stream channel | Datagram channel |
|---|---|---|
| **QUIC primitive** | bidi stream (`connection.open_bi`) | virtual stream (= `channel_id` で識別、 connection 共有 datagram path) |
| **物理的単位** | 1 QUIC stream | varint `channel_id` prefix |
| **配送保証** | Ordered + Reliable | Unordered + Unreliable |
| **サイズ制約** | 無制限 (= stream で chunk 可) | ≤ MTU (= 1300B 安全値) |
| **HoL blocking** | Channel 内 blocking 許容 | なし |

「channel は 1 stream に相当する通信路」 という mental model は backend を超えて維持され
る。 datagram の virtual stream identity は `channel_id` (= author 明示割り当て)。

### 2.2 1 channel = 1 backend (strict)

v0.10.0 では 1 KDL channel block 内の event は **全て同じ backend** に従う (= mixed
不可)。 reasons:

1. **型 safety**: `DatagramChannel<C>` と `UnisonChannel<C>` を別型分離、 datagram-only
   API (= MTU check 等) を型 surface で expose
2. **spec coherency**: 「channel = 1 backend を持つ通信路」 という cleaner definition
3. **Forward-compatibility**: strict → mixed は非破壊で許容化可能、 逆は breaking
4. **実装シンプル**: channel 内部は単一 wire format、 hybrid 構造不要

v0.11+ で実 use case (= 同 channel に control + telemetry 等) が蓄積したら mixed 許容
を再評価。

---

## 3. KDL syntax

### 3.1 Channel block 拡張

```kdl
channel "<name>" from="<direction>" lifetime="<lifetime>" [backend="<backend>"] [channel_id=<N>] {
    event "<EventName>" { ... }
    // request/response は backend="stream" でのみ許可
}
```

新規 attribute:

| 属性 | 値 | 必須条件 |
|------|-----|---------|
| `backend` | `"stream"` (default) / `"datagram"` | 任意 (省略時 `"stream"`) |
| `channel_id` | `1..` の正整数 | `backend="datagram"` 時のみ必須 |

### 3.2 Channel ID 割り当て規約

- **Explicit only**: KDL で `channel_id=N` を author が明示指定 (= proto3 field number 哲学)
- **Wire compatibility**: schema reorder で channel_id 変えると wire format breaking、
  reorder OK だが ID は変えない規約
- **Conflict detection**: KDL parser が同一 namespace 内の channel_id 重複を error
- **Reserved range**: 0 は予約 (= 後方互換 / sentinel)、 1.. が author 利用領域

### 3.3 例

```kdl
protocol "vp-sync" version="0.1.0" {
    namespace "club.chronista.vp"

    // Stream channel (= default backend、 channel_id 不要)
    channel "control" from="either" lifetime="persistent" {
        request "Subscribe" {
            field "topic" type="string"
            returns "Subscribed" {
                field "ok" type="bool"
            }
        }
    }

    // Datagram channel (= backend 明示、 channel_id 必須)
    channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
        event "Transform" {
            field "id" type="string"
            field "pos" type="json"
            field "rot" type="json"
        }
    }

    channel "presence" from="either" lifetime="persistent" backend="datagram" channel_id=2 {
        event "Heartbeat" {
            field "user_id" type="string"
            field "ts" type="timestamp"
        }
    }
}
```

---

## 4. Wire format

### 4.1 Datagram payload layout

```text
[varint channel_id] [buffa-encoded event message]
```

- 先頭 1-2 byte が varint encoded `channel_id` (= proto3 LEB128 と同じ encoding)
- 残りが buffa (protobuf) でエンコードされた event 本体
- 1 datagram = 1 event message、 chunking / fragmentation 不可

### 4.2 Channel ID 1-127 の hot path

`channel_id` が 1-127 の範囲なら varint prefix は **1 byte**、 これは hot path (= 3DCG
transform 大量配信) で最も使用頻度が高い帯域。 MTU 1300B の 0.08% を prefix が占有。

128 以上は 2 byte となるが、 一般的な use case では 100+ datagram channel は稀。

### 4.3 Demux 処理

受信側:

1. QUIC datagram frame 受信
2. payload 先頭の varint をデコードして `channel_id` を取得
3. `channel_id` を key に handler dispatch table を lookup
4. 残りの payload を buffa decode → event handler invoke

性能 hot path:

- varint decode: 1 read + 1-2 branch、 ns オーダー
- handler dispatch: HashMap lookup or const array index、 ns オーダー
- buffa decode: zero-copy `*View` 型を使えば allocation ゼロ

### 4.4 MTU 超過時の挙動

`SendDatagramError::TooLarge` が caller に返る。 fragment / chunk 不可。 caller は:

- payload を分割して複数 datagram に分けて送る (= application-level fragmentation)
- 大きい payload は stream channel に逃がす
- buffa zero-copy view を活用して message size を最小化

---

## 5. Type API

### 5.1 DatagramChannel<C>

```rust
pub struct DatagramChannel<C: Codec = JsonCodec> {
    // private impl
}

impl<C: Codec> DatagramChannel<C> {
    /// Event を送信 (= per-connection、 channel direction が許す方向)
    pub async fn send_event<T: Encodable<C>>(&self, event: &T) -> Result<(), NetworkError>;

    /// Event を受信
    pub async fn recv_event<T: Decodable<C>>(&self) -> Result<T, NetworkError>;

    /// Channel ID (= schema 由来)
    pub fn channel_id(&self) -> u64;

    /// Channel close
    pub async fn close(&self) -> Result<(), NetworkError>;
}
```

`UnisonChannel<C>` (= stream channel) との **共通点**: codec generic、 `send_event` /
`recv_event` 持つ。 **相違点**: `request` / `send_response` なし (= datagram は
request/response 不適合)、 `channel_id` getter あり、 `close` semantics 異なる (= stream
は FIN、 datagram は handler 登録解除のみ)。

### 5.2 Server-level broadcast

```rust
impl ProtocolServer {
    /// Datagram channel handler 登録 (= name + channel_id + handler factory)
    pub async fn register_channel_datagram<F, Fut>(&self, name: &str, channel_id: u64, handler: F)
    where
        F: Fn(DatagramChannel<JsonCodec>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static;

    /// 全 connected client へ datagram channel event を broadcast
    pub async fn broadcast<T, C>(&self, channel_name: &str, event: &T) -> Result<usize, NetworkError>
    where
        T: Encodable<C>,
        C: Codec;
}
```

`broadcast` の戻り値は配送成功した client 数 (= datagram は drop 許容なので best-effort)。
`register_channel_datagram` は default codec = JsonCodec、 別 codec が必要な場合は将来
`register_channel_datagram_with<C>` を追加予定 (v0.11+)。

### 5.3 Client-side open

```rust
impl ProtocolClient {
    /// Datagram channel を open (default codec = JsonCodec)
    pub async fn open_datagram_channel(
        &self,
        channel_name: &str,
        channel_id: u64,
    ) -> Result<DatagramChannel<JsonCodec>, NetworkError>;

    /// Datagram channel を open (任意 codec 指定版)
    pub async fn open_datagram_channel_with<C: Codec>(
        &self,
        channel_name: &str,
        channel_id: u64,
    ) -> Result<DatagramChannel<C>, NetworkError>;
}
```

`channel_id` は KDL schema 由来 (= author 明示割り当て)、 codegen が `open_datagram_channel(name, channel_id)` の形で生成する。 Rust の制約で method-level generic default は不可 (= `<C: Codec = JsonCodec>` 不可)、 そのため default 版と generic 版を 2 method で提供。

---

## 6. Migration: v0.9.0 connection-level → v0.10.0 channel API

v0.9.0 の `QuicClient::{send_datagram, recv_datagram}` は v0.10.0 でも残存 (= escape
hatch)、 ただし **新規 caller には datagram channel API を推奨**。

### 6.1 比較表

| 観点 | v0.9.0 connection-level | v0.10.0 channel API |
|------|------------------------|---------------------|
| Demux | caller が payload header で実装 | library が `channel_id` varint prefix で自動 |
| 型 safety | raw `Bytes` | buffa-encoded typed `T` |
| Server handler | accept loop 自前 | `register_channel_datagram` で declarative |
| Broadcast | per-connection iterate | `server.broadcast` 1 行 |
| Schema | application-level protocol | KDL schema-first |

### 6.2 移行例

**Before (v0.9.0、 raw bytes)**:

```rust
// Client
let bytes = bincode::serialize(&transform)?;
client.send_datagram(bytes.into()).await?;

// Server (custom accept loop)
let connection = server.accept().await?;
loop {
    let bytes = connection.read_datagram().await?;
    let transform: Transform = bincode::deserialize(&bytes)?;
    // ...
}
```

**After (v0.10.0、 channel API)**:

```rust
// KDL schema 経由で DatagramChannel<ProtoCodec> 生成

// Client
let chan = client.open_datagram_channel("position").await?;
chan.send_event(&transform).await?;

// Server
server.register_channel_datagram("position", |chan| async move {
    loop {
        let transform: Transform = chan.recv_event().await?;
        // ...
    }
}).await;

// Broadcast (= server 主導 push)
server.broadcast("position", &transform).await?;
```

---

## 7. v0.11+ 引き継ぎ

- **Mixed backend channel 許容検討** (= 同 KDL channel block に stream / datagram event 共存)
- **Subscription model**: client が subscribe 宣言、 server-side filter で per-client filtering
- **Datagram channel の bench 拡充**: channel API 経由の demux overhead 計測
- **Channel negotiation**: handshake で wire format / backend を動的合意
- **MTU 動的調整**: PMTU discovery 結果を schema-level に反映、 fragmentation 戦略の library 化

---

## 8. 関連

- [spec/02-unified-channel/SPEC.md](../spec/02-unified-channel/SPEC.md) §4.4 (= KDL syntax)、 §8.5 (= datagram channel narrative)
- [design/wire-format.md](./wire-format.md) — buffa wire format (= datagram event payload も同じ buffa encoding)
- [design/architecture.md](./architecture.md) — overall network layer (要 update)
- creo-memories: `mem_1Cb46rfagUw7Qqc9WthSsP` (= 本 task の memo)
