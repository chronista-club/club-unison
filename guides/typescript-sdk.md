# TypeScript SDK — API リファレンス

`@chronista-club/unison-client` の公開 API リファレンス。KDL スキーマから
typed クライアントを得るまでの流れと、各 API の使い方を例で示す。

実装の設計契約は [`design/typescript-client-api.md`](../design/typescript-client-api.md)、
end-to-end の動かし方は [`quickstart.md`](quickstart.md) を参照。

---

## 1. KDL スキーマ → codegen → typed クライアント

Unison は **KDL スキーマが型の SSOT**。スキーマから言語別のコードを生成する。

```text
KDL schema  ──codegen──▶  TS types + ChannelMeta const  ──SDK──▶  typed client
```

### codegen

コード生成は `club-kdl-codegen` crate（crates.io 公開）が担う。CLI で TS 型を生成:

```bash
cargo install club-kdl-codegen
club-kdl-codegen schemas/protocol.kdl --target typescript > generated/protocol.ts
```

生成物は channel ごとに次を吐く:

- event / request payload の `interface`
- 型を運ぶ `<Channel>ChannelEventTypes` / `<Channel>ChannelRequestTypes`
- `as const` の `ChannelMeta`（後述の `__types` phantom carrier 込み）

> v1.0 時点の codegen は **stream channel まで**。datagram channel の TS
> codegen と proto descriptor の自動生成は v1.x deferred。datagram の
> `ChannelMeta` は当面手書きする（[example](../clients/typescript/examples/vp-dashboard.ts) 参照）。

### ChannelMeta を手書きする

codegen を使わず手で書くこともできる。`as const satisfies ChannelMeta` で
型チェックする:

```typescript
import type { ChannelMeta } from "@chronista-club/unison-client";

const EchoMeta = {
  name: "echo",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: [],
  requests: {
    Echo: { request: "EchoReq", response: "EchoResp" },
  },
} as const satisfies ChannelMeta;
```

event payload / request payload を typed にするには `__types` phantom carrier
を付ける（runtime 値は `undefined`、型のみ運ぶ）:

```typescript
interface EchoReq { text: string }
interface EchoResp { text: string }

const EchoMeta = {
  name: "echo", backend: "stream", from: "client", lifetime: "persistent",
  events: [],
  requests: { Echo: { request: "EchoReq", response: "EchoResp" } },
  __types: undefined as unknown as {
    events: Record<string, never>;
    requests: { Echo: { request: EchoReq; response: EchoResp } };
  },
} as const satisfies ChannelMeta;
```

`__types` を持たない素の `ChannelMeta` は payload が `ChannelPayload`
（`Record<string, unknown>`）に degrade する。

---

## 2. `connect()` — 接続を確立する

`connect()` がクライアント側の primary entry。`UnisonClient` を返す。

```typescript
import { connect } from "@chronista-club/unison-client";

const client = await connect({
  url: "https://host:port",       // WebTransport は https-only
  trust: "system",                // または { certHash: "<64 hex>" }
  awaitIdentity: true,            // default、identity handshake を待つ
});
```

### `UnisonConnectOptions`

| フィールド | 型 | default | 説明 |
|---|---|---|---|
| `url` | `string` | — | 接続先 URL（https-only） |
| `trust` | `TrustMode` | `"system"` | TLS trust policy |
| `signal` | `AbortSignal` | — | 接続の kill-switch |
| `transport` | `Transport` | WebTransport | 通常は省略。test で mock を注入 |
| `codec` | `Codec` | `JsonCodec` | 全 channel 共有 payload codec |
| `awaitIdentity` | `boolean` | `true` | identity handshake を待つか |
| `identityTimeoutMs` | `number` | `5000` | identity handshake の timeout |

### `TrustMode`

```typescript
type TrustMode =
  | "system"               // システム CA store による標準検証
  | { certHash: string };  // 自己署名証明書を SHA-256 で pin（hex 文字列）
```

cert pinning は **loopback host 限定**（`localhost` / `127.0.0.1` / `[::1]`）。
非 loopback への pinning は例外を投げる。

---

## 3. `UnisonClient`

`connect()` が返す facade。1 connection を束ね、配下に channel を開設する。

```typescript
interface UnisonClient {
  serverIdentity(): ServerIdentity | undefined;
  events(): AsyncIterable<ConnectionEvent>;
  openChannel<M>(meta: M, openTimeoutMs?: number): Promise<UnisonChannel<M>>;
  openDatagramChannel<M>(meta: M): DatagramChannel<M>;
  disconnect(reason?: string): Promise<void>;
}
```

### `serverIdentity()`

connect 時の identity handshake で受信したサーバ自己紹介。`awaitIdentity: false`
や handshake が来なかった場合は `undefined`:

```typescript
const id = client.serverIdentity();
if (id !== undefined) console.log(`${id.name} v${id.version}`);
```

### `events()` — connection lifecycle

```typescript
for await (const ev of client.events()) {
  if (ev.type === "connected") console.log(`connected: ${ev.remoteAddr}`);
  else if (ev.type === "disconnected") console.warn(`down: ${ev.reason}`);
  else if (ev.type === "error") console.error(ev.error);
}
```

**library は auto-reconnect しない**。`disconnected` を見て caller が自前の
ポリシーで `connect()` を再呼び出しする責務を持つ。

### `openChannel()` / `openDatagramChannel()`

`openChannel()` は bidi stream を 1 本開き、**open handshake**（`open` frame →
server `open_ack`）でサーバが accept したことを確認してから resolve する。
`openTimeoutMs` 内に accept されなければ reject + stream tear down。

`openDatagramChannel()` は同期。共有 datagram path 上の virtual channel を返す。

---

## 4. `UnisonChannel` — stream channel

```typescript
interface UnisonChannel<M> {
  request<N>(name: N, payload): Promise<ResponseType<M, N>>;
  events(): AsyncIterableIterator<EventType<M>>;
  sendEvent<N>(name: N, payload): Promise<void>;
  close(): Promise<void>;
}
```

### request/response

```typescript
const echo = await client.openChannel(EchoMeta);
const reply = await echo.request("Echo", { text: "hi" });
// reply は EchoResp 型に narrow（__types 経由）
```

### event subscribe

```typescript
for await (const ev of echo.events()) {
  // ev は EchoMeta の event payload union 型
  // break で iterator が閉じ、channel close へ cascade
}
```

### event 送信（client → server）

`from` が `"client"` / `"either"` の channel で使える:

```typescript
await chan.sendEvent("SomeEvent", { ... });
```

---

## 5. `DatagramChannel` — datagram channel

unordered + unreliable な broadcast event 用。`request` は持たない。

```typescript
interface DatagramChannel<M> {
  readonly channelId: M["channelId"];
  events(): AsyncIterableIterator<EventType<M>>;
  sendEvent<N>(name: N, payload): Promise<void>;
  close(): Promise<void>;
}
```

```typescript
const metricChan = client.openDatagramChannel(MetricChannelMeta);
for await (const update of metricChan.events()) {
  dashboardStore.set(update.name, update.value);
}
```

datagram の wire レイアウトは `[varint channel_id][codec-encoded payload]`。
1 datagram = 1 event、chunking 不可。**MTU を超えると drop される**点に注意
（JSON codec の `Vec<u8>` encode は wire size が膨らみやすく、effective
payload limit は概ね 200-300 B）。

---

## 6. Codec

```typescript
interface Codec<T> {
  readonly format: CodecFormat;   // "json" | "proto"
  encode(value: T): Uint8Array;
  decode(bytes: Uint8Array): T;
}
```

| codec | クラス | 説明 |
|---|---|---|
| JSON | `JsonCodec` | default。構造的に任意の値を扱える。`JsonCodec.shared` で共有 instance |
| protobuf | `ProtoCodec` | buf protobuf（`@bufbuild/protobuf`）。descriptor 駆動 |

codec は connection-level で一律指定する（`connect({ codec })`）。
per-channel override は v1.x deferred。

```typescript
import { connect, JsonCodec } from "@chronista-club/unison-client";
const client = await connect({ url: "...", codec: JsonCodec.shared });
```

> `ProtoCodec` は利用可能だが、KDL → proto descriptor の自動 codegen は
> v1.0 では未対応。proto を使うには descriptor を別途用意する必要がある。

---

## 7. `ErrorCategory` — エラー分類

boundary error を programmatic に判定するための分類。Rust 側
`unison::ErrorCategory`（snake_case シリアライズ）と一致する 4 値:

```typescript
type ErrorCategory = "transport" | "protocol" | "application" | "resource";
```

| 値 | 層 |
|---|---|
| `transport` | QUIC / TLS / DNS |
| `protocol` | 不正パケット / スキーマ不整合 / channel 状態 |
| `application` | caller / handler が返したエラー |
| `resource` | quota / rate-limit / timeout |

全値は `ERROR_CATEGORIES` で取得できる。

---

## 8. wire-format ユーティリティ（低レベル）

SDK は Rust サーバと互換な wire format の encode/decode 関数も公開している
（`encodePacket` / `decodePacket` / `encodeProtocolMessage` /
`decodeProtocolMessage` / frame 系）。これらは通常 caller が直接触る必要は
なく、`UnisonChannel` / `DatagramChannel` が内部で使う。独自トランスポートを
書く / wire を検証する等の高度な用途向け。

---

## 9. 公開シンボル一覧

`@chronista-club/unison-client` から export される主要シンボル:

| カテゴリ | シンボル |
|---|---|
| Top-level | `connect`, `UnisonClient`, `UnisonConnectOptions`, `VERSION` |
| Channel 型 | `ChannelMeta`, `DatagramChannelMeta`, `UnisonChannel`, `DatagramChannel`, `EventType`, `RequestType`, `ResponseType`, `ChannelPayload` |
| Transport | `Transport`, `Connection`, `ConnectOptions`, `ConnectionEvent`, `TrustMode`, `BidiStream` |
| Codec | `Codec`, `CodecFormat`, `JsonCodec`, `ProtoCodec`, `CodecError` |
| Error | `ErrorCategory`, `ERROR_CATEGORIES` |
| Identity | `ServerIdentity`, `ChannelInfo`, `ChannelStatus`, `ChannelDirection` |
| 低レベル wire | `encodePacket` / `decodePacket`, `encodeProtocolMessage` / `decodeProtocolMessage`, frame 系 |

---

## 10. Rust API との対応

| 機能 | Rust | TypeScript |
|---|---|---|
| 接続 | `client.connect(url)` | `connect({ url, trust })` |
| connection events | `client.subscribe_connection_events()` | `client.events()` |
| stream channel open | `client.open_channel("name")` | `client.openChannel(meta)` |
| datagram channel open | `client.open_datagram_channel("name", id)` | `client.openDatagramChannel(meta)` |
| request | `chan.request::<Req, Resp>("Name", &req)` | `chan.request("Name", payload)` |
| event 受信 | `loop { chan.recv().await }` | `for await (e of chan.events())` |
| event 送信 | `chan.send_event(...)` | `chan.sendEvent("Name", payload)` |
| 切断 | `client.disconnect()` | `client.disconnect()` |

TS は `ChannelMeta` の `as const` literal narrowing で、Rust の turbofish
（`request::<Req, Resp>`）相当の型推論を引数だけで実現する。

---

**最終更新**: 2026-05-17（v1.0）
