# design/typescript-client-api.md — TS Client SDK API Design (= v1.0 Phase 3a deliverable)

**バージョン**: 1.0 (= package `@chronista-club/unison-client` 1.2.0 の実装に追従)
**最終更新**: 2026-11-04
**ステータス**: Implemented (`clients/typescript`)。 §3 と §5 は実装前の設計 (歴史)、 §4 は現在の export に合わせてある

---

## 1. 目的

「**ideal caller TS code を先に sketch して、 Phase 2 SDK の API surface を逆設計する**」 ための design exercise。 v0.10.0 で Rust 側で取った top-down approach (= codegen 出力 1f → 実装 1d/1e) を polyglot で再現。

> Phase 2 は package 1.2.0 までに実装済み (§12)。 §3 (caller sketch) と §5 (codec 戦略) は
> **実装前の設計 (歴史)** として当時の形のまま残し、 実装と違う点は各節の冒頭に注記する。
> §4 の API 契約は現在の `clients/typescript/src/index.ts` の export に合わせてある。

## 2. 想定 caller use case (= Vantage Point dashboard subscribe)

### 2.1 シナリオ

Vantage Point は chronista-club ecosystem の AI native dev environment、 Canvas (= 視覚化 dashboard) を持つ。 現状の dashboard data refresh は静的 HTML + WASM 構成、 realtime data 取得は限定的。

v1.0 TS client SDK 採用後、 **Canvas が unison server に直接 subscribe** して metric / agent status / build progress を realtime push 受信する形に。 期待効果:

- REST polling → unison datagram subscribe で latency 1 秒 → 数 ms
- gateway layer (= REST API 変換) 削減、 maintain cost 低下
- type-safe (= server schema が TS interface に直接 propagate)

### 2.2 KDL schema 仮想例

```kdl
protocol "vp-dashboard" version="1.0.0" {
    namespace "club.chronista.vp"

    // Dashboard metric の datagram broadcast (= 60Hz refresh 想定)
    channel "metric" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
        event "MetricUpdate" {
            field "name" type="string" required=#true
            field "value" type="float" required=#true
            field "unit" type="string"
            field "ts" type="timestamp"
        }
    }

    // Agent status (= less frequent、 stream channel で reliable)
    channel "agent_status" from="server" lifetime="persistent" {
        event "AgentEvent" {
            field "agent_id" type="string" required=#true
            field "status" type="string" required=#true
            field "details" type="json"
        }
    }

    // Dashboard control (= client → server、 request/response)
    channel "control" from="client" lifetime="persistent" {
        request "SubscribeMetric" {
            field "names" type="array"
            returns "Subscribed" {
                field "ok" type="bool"
            }
        }
    }
}
```

---

## 3. Ideal caller TS code (= 「これで dashboard 書きたい」)

> **実装前の設計 (歴史)**。 実装との差 (現在の API は §4):
> - entry は `unisonClient.connect` ではなく関数 `connect` (`import { connect } from "@chronista-club/unison-client"`)、 戻り値は class `UnisonClient`
> - `trust` は `"system" | { certHash: string }`。 browser の WebTransport に検証 skip は無いので `"skip-verify"` は存在しない
> - `subscribeConnectionEvents()` / `events.on(...)` ではなく `client.events()` を `for await` で読む
> - `serverIdentity()` は Promise ではなく、 connect 時に受けた `ServerIdentity | undefined` を同期で返す
> - `openDatagramChannel` は Promise を返さない (同期で `DatagramChannel` を返す)

### 3.1 Connection setup

```typescript
import { unisonClient } from "@chronista-club/unison-client";

// Connection establish (= async、 mTLS / WebTransport setup 込み)
const client = await unisonClient.connect({
  url: "https://vp.chronista.local:8080",
  trust: "system",  // or "skip-verify" for dev_localhost
});

// Connection lifecycle event を subscribe (= server side と parallel API)
const events = client.subscribeConnectionEvents();
events.on("connected", ({ remoteAddr }) => console.log(`connected to ${remoteAddr}`));
events.on("disconnected", ({ reason }) => {
  console.warn(`disconnected: ${reason}`);
  // caller がここで自前 reconnect ロジック (= library は auto-reconnect しない)
});

// Server identity (= post connect、 Rust 側と同様)
const identity = await client.serverIdentity();
console.log(`connected to ${identity.name} v${identity.version}`);
```

### 3.2 Datagram channel subscribe (= 主用途)

```typescript
import { MetricChannelMeta, type MetricUpdate } from "./generated/vp-protocol";

// Datagram channel open (= type-safe、 MetricChannelMeta が compile-time に
// channel_id / event types を narrow)
const metricChan = await client.openDatagramChannel(MetricChannelMeta);

// Event subscribe (= AsyncIterable、 60Hz の steady stream)
for await (const update of metricChan.events()) {
  // update は MetricUpdate 型に narrow (= ChannelMeta.events["MetricUpdate"] で型導出)
  dashboardStore.set(update.name, update.value);
  // unit / ts も typed
}

// Server side から push 一方向、 client 側は subscribe のみ (= datagram channel の典型)
```

### 3.3 Stream channel + request/response (= control 用)

```typescript
import { ControlChannelMeta } from "./generated/vp-protocol";

const controlChan = await client.openChannel(ControlChannelMeta);

// Request/response (= 型 narrowing で response 型自動推論)
const result = await controlChan.request("SubscribeMetric", {
  names: ["cpu", "memory", "build_progress"],
});
// result は Subscribed 型 (= channel meta.requests.SubscribeMetric.response から narrow)

if (result.ok) {
  console.log("subscribed");
}
```

### 3.4 Stream channel + event subscribe (= less frequent push)

```typescript
import { AgentStatusChannelMeta, type AgentEvent } from "./generated/vp-protocol";

const agentChan = await client.openChannel(AgentStatusChannelMeta);

for await (const event of agentChan.events()) {
  // event は AgentEvent 型
  if (event.status === "completed") {
    notifyAgentCompleted(event.agent_id);
  }
}
```

### 3.5 Cleanup

```typescript
// Channel close (= caller 責務)
await metricChan.close();
await controlChan.close();
await agentChan.close();

// Connection close
await client.disconnect();
```

---

## 4. SDK API surface (= 現在の export、 `clients/typescript/src/index.ts`)

当初は Phase 2 の implementation contract として書いた節。 package 1.2.0 の実装に合わせて
更新してある。

### 4.1 Top-level API

```typescript
// transport/types.ts
export type TrustMode = "system" | { certHash: string }; // certHash = DER server cert の SHA-256 (hex)

export interface ConnectOptions {
  url: string;          // https://host:port (WebTransport は https のみ)
  trust?: TrustMode;    // 既定 "system"
  signal?: AbortSignal; // abort で connect を中断、 確立後は connection と配下の stream を tear down
}

// client.ts
export interface UnisonConnectOptions extends ConnectOptions {
  transport?: Transport;          // 既定 WebTransport。 test では mock transport を注入する
  codec?: Codec<ChannelPayload>;  // 全 channel 共有の codec (= JsonCodec / ProtoCodec のインスタンス)。 既定 JsonCodec
  awaitIdentity?: boolean;        // identity handshake を待つか (既定 true)
  identityTimeoutMs?: number;     // identity handshake の timeout (既定 5000)
  credential?: Uint8Array;        // connection-level auth (§4.5)
}

export function connect(opts: UnisonConnectOptions): Promise<UnisonClient>;

export class UnisonClient {
  // connect 時に受けた identity。 awaitIdentity: false / handshake が来なかった場合は undefined
  serverIdentity(): ServerIdentity | undefined;
  // connection lifecycle event (§4.4)
  events(): AsyncIterable<ConnectionEvent>;
  // openChannel は open handshake (= `open` frame → server `open_ack`) を行い、
  // server peer が bidi stream を accept したことを確認してから resolve する。
  // `openTimeoutMs` (既定 5000) 内に accept されなければ reject + stream tear down (= no-accept signal)。
  openChannel<M extends ChannelMeta>(meta: M, openTimeoutMs?: number): Promise<UnisonChannel<M>>;
  openDatagramChannel<M extends DatagramChannelMeta>(meta: M): DatagramChannel<M>;
  authenticate(credential: Uint8Array): Promise<void>; // §4.5
  disconnect(reason?: string): Promise<void>;
}
```

- library は auto-reconnect しない (= caller 責務)
- browser の WebTransport は「cert 検証を全部 skip」を持たないので、 CA 以外の経路は
  `{ certHash }` の pinning (= `serverCertificateHashes`) だけ
- identity handshake は best-effort。 失敗しても connection は使え、 `serverIdentity()` が
  `undefined` を返す

caller-facing の connection entry は **free function `connect`** で export する
(= `import { connect } from "@chronista-club/unison-client"`)。 transport-level の
低レベル connect は `connectTransport` として別 export、 facade と名前衝突しない。

### 4.2 Channel API

```typescript
// Stream channel
export interface UnisonChannel<M extends ChannelMeta = ChannelMeta> {
  readonly name: M["name"];

  // Request → Response (= meta.requests[N].request / response 型推論)
  request<N extends RequestName<M>>(
    name: N,
    payload: RequestType<M, N>,
  ): Promise<ResponseType<M, N>>;

  // Event subscribe (= server push、 `for await`、 break で channel close cascade)
  events(): AsyncIterableIterator<EventType<M>>;

  // Event send (= client → server、 応答なし)
  sendEvent<N extends EventName<M>>(name: N, payload: EventPayload<M, N>): Promise<void>;

  close(): Promise<void>; // idempotent
}

// Datagram channel (= event only、 request 不可)
export interface DatagramChannel<M extends DatagramChannelMeta = DatagramChannelMeta> {
  readonly name: M["name"];
  readonly channelId: M["channelId"]; // = compile-time const (from meta)
  events(): AsyncIterableIterator<EventType<M>>;
  sendEvent<N extends EventName<M>>(name: N, payload: EventPayload<M, N>): Promise<void>; // MTU 超過は reject
  close(): Promise<void>; // dispatcher から unregister、 idempotent
}
```

### 4.3 ChannelMeta type narrowing

Phase 1 で生成した `as const` channel meta を使う。 meta const は文字列 literal
しか保持できないため、 codegen は **phantom `__types` carrier** を埋め込む:
event/request 名 → 生成 interface の map を `__types` field に型として載せる
(= runtime 値は `undefined`)。 `EventType<M>` / `RequestType<M,N>` /
`ResponseType<M,N>` はこの carrier 経由で実 interface を解決する。

```typescript
// Phase 1 generated:
export interface MetricUpdate { name: string; value: number; unit?: string }
export interface MetricChannelEventTypes { MetricUpdate: MetricUpdate }
export type MetricChannelRequestTypes = Record<string, never>;

export const MetricChannelMeta = {
  name: "metric" as const,
  backend: "datagram" as const,
  channelId: 1 as const,
  from: "server" as const,
  lifetime: "persistent" as const,
  events: ["MetricUpdate"] as const,
  requests: {} as const,
  // phantom carrier — runtime undefined、 型のみ
  __types: undefined as unknown as {
    events: MetricChannelEventTypes;
    requests: MetricChannelRequestTypes;
  },
} as const;

// type narrowing (channel/types.ts):
type EventName<M> = M extends { events: readonly (infer N)[] } ? N & string : never;
type EventType<M> = // __types.events 経由で生成 interface に解決
  M extends { __types?: infer T }
    ? T extends ChannelTypeMap
      ? T["events"][keyof T["events"]]
      : ChannelPayload
    : ChannelPayload;

// 結果: caller code で
const chan = client.openDatagramChannel(MetricChannelMeta);
for await (const update of chan.events()) {
  // update: MetricUpdate (= compile-time に narrow、 手動 cast 不要)
}
```

`__types` を持たない raw `ChannelMeta` で開いた場合は `ChannelPayload`
(= `Record<string, unknown>`) に degrade する (= 後方互換)。

### 4.4 Connection event API

```typescript
// transport/types.ts
export type ConnectionEvent =
  | { type: "connected"; remoteAddr: string }
  | { type: "disconnected"; reason: string }
  | { type: "error"; error: Error };

for await (const e of client.events()) {
  if (e.type === "disconnected") {
    // caller がここで自前 reconnect ロジック (= library は auto-reconnect しない)
  }
}
```

emitter (`on(...)`) ではなく AsyncIterable にしたのは、 event subscribe (§8.3) と同じ
`for await` の形に揃えるため。

### 4.5 Auth API (connection-level、 v1.4.0)

connection-level auth primitive（Rust `enable_auth` / `connect_with_credential`、設計
`design/connection-auth.md`）の TS 対応。auth は `unison.auth` channel を open して
`Authenticate` request を送るだけなので、専用 transport は不要。

API は §4.1 の 2 つ:

- `connect({ ..., credential })` — connect-time auth (= Rust `connect_with_credential` の対応物)。
  identity handshake 後に `unison.auth` で Authenticate し、 拒否 (ok=false) なら connection を
  畳んで throw する
- `client.authenticate(credential)` — post-connect / re-auth 用。 `unison.auth` を open して
  Authenticate を送り、 ok=false なら throw。 `connect({ credential })` は内部でこれを呼ぶ

**wire 不変条件**（`design/connection-auth.md` §5.8 が SSOT）:
- request payload = `{ credential: number[] }`。`Uint8Array` は **`Array.from(bytes)` で `number[]`
  に変換**してから送る（直接入れると JSON が `{"0":..}` object 化して Rust の `Vec<u8>` と非互換）。
- response = `{ ok: boolean }`。`ok === false` → `authenticate` / `connect` が throw。

reserved channel meta（`AUTH_CHANNEL_META`）と定数（`AUTH_CHANNEL_NAME` = `"unison.auth"` /
`AUTHENTICATE_METHOD` = `"Authenticate"`）、 credential を wire 形に変える `toAuthenticateRequest` は
SDK が内蔵し（`src/channel/auth.ts`）、codegen 不要。

---

## 5. Codec 戦略

> **実装前の設計 (歴史)**。 実装との差: `codec` は `"json" | "proto"` の文字列ではなく
> `Codec<ChannelPayload>` のインスタンスを渡す (既定は `JsonCodec.shared`、 §4.1)。
> `ProtoCodec` は 1 つの message descriptor に束縛される (`new ProtoCodec(Schema)`) ため、
> connection-level で一律に渡せるのは全 channel が同じ message を運ぶ場合に限られる。
> per-channel の codec 指定は未実装。

### 5.1 Default = JsonCodec

- 全 channel が default で JSON wire (= caller の opt-out なし設定が simple)
- `unisonClient.connect()` で `codec: "json"` (default) / `codec: "proto"` 選択
- 各 channel ごとの codec 指定は v1.x で検討

### 5.2 ProtoCodec opt-in

```typescript
const client = await unisonClient.connect({
  url: "...",
  codec: "proto",  // 全 channel で ProtoCodec、 @bufbuild/protobuf 経由
});
```

ProtoCodec 利用条件:
- KDL schema が proto3 互換 (= field number 等)
- 生成された TS code に proto descriptor が含まれる (= Phase 2d で codegen 拡張)

### 5.3 Per-channel codec override (= v1.x deferred)

Phase 2 では「connection-level codec 一律」 のみ実装、 channel 別 codec は v1.x で caller demand 駆動。

---

## 6. Bundle / Build tooling

### 6.1 Bundler choice = vite

理由:
- ESM-first、 modern browser target に natural
- Dev server が高速 (= esbuild base)
- production build (= rollup base) で tree-shake 優秀
- vitest と integration、 test 環境 unified

### 6.2 Bundle size target

| 要素 | 目標 size | 備考 |
|---|---|---|
| Core SDK (= transport + channel + json codec) | ≤ 100 KB minified gzipped | mandatory baseline |
| Core + ProtoCodec (= @bufbuild/protobuf 含む) | ≤ 200 KB minified gzipped | opt-in、 caller が import した時のみ |
| Type definitions (= .d.ts) | 別 file、 runtime 影響なし | |

### 6.3 Tree-shake friendly

- `import { connect } from "@chronista-club/unison-client"` 時、 caller が使わない codec / transport adapter は dead-code elim
- Sub-path import 検討: transport だけを sub-path で出す案。 現在の sub-path は `@chronista-club/unison-client/testing` (mock transport / server stub) のみ
- size check CI gate (= bundle が threshold 超えたら fail) は未導入。 Phase 2e では build と size の計測まで (minified 56.3 KB / gzipped 14.7 KB、 2026-05-16)

---

## 7. WebTransport polyfill 戦略

### 7.1 Browser compatibility 現状 (= 2026-05 時点)

| Browser | WebTransport native | 状態 |
|---|---|---|
| Chromium 95+ / Edge / Opera | ✅ full | production |
| Safari 18+ | 🟡 partial | iOS 18 限定で datagram OK、 stream は実装中 |
| Firefox 135+ | 🟡 behind flag | nightly で動作確認可、 default false |

### 7.2 Phase 2 では native のみ、 polyfill は v1.x

v1.0 では:
- **Chromium 系のみ official support** (= Vantage Point dashboard は Chrome dev 想定で OK)
- Safari / Firefox は「caller が WebTransport flag を有効化 / wait for native」
- polyfill (= WebSocket fallback) は v1.x で chronista-club 外 caller 出現後に検討

### 7.3 Detection / graceful degradation

```typescript
const client = await connect({ url: "..." });
// 内部で `typeof WebTransport === "undefined"` check
// 不可なら明示 error: `WebTransportUnsupportedError`
// (= silent fail せず、 caller に clear signal)
```

---

## 8. TypeScript-specific design decisions

### 8.1 ESM only (= no CommonJS)

理由:
- Modern target (= browsers + Node.js 18+) は ESM native
- CJS 同梱は bundle size + maintenance cost
- caller dev tool (= vite / next / etc.) は ESM 前提

### 8.2 Strict TypeScript

```json
// tsconfig.json (= Phase 2a で commit)
{
  "compilerOptions": {
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "exactOptionalPropertyTypes": true,
    "target": "ES2022",
    "module": "ESNext",
    "lib": ["DOM", "ES2022", "DOM.AsyncIterable"]
  }
}
```

### 8.3 AsyncIterable for event subscribe

`for await (const e of chan.events())` pattern を採用:
- Browser / Node.js 両方 native support
- backpressure / cleanup 自動 (= break で iterator close → channel close cascade)
- Observable / EventEmitter pattern より async-flow が clean

### 8.4 Generated code は別 directory

```
generated/
├── vp-protocol.ts       ← Phase 1 codegen 出力 (= types + ChannelMeta const)
└── ...

src/                     ← SDK 実装 (Phase 2b/c/d/e)
├── transport/
├── channel/
├── codec/
└── index.ts
```

`generated/` は user の `tsconfig.json` で include、 SDK は import path で reference。

---

## 9. Phase 2 implementation sub-phase 詳細

design doc が確定したら Phase 2b/c/d/e の具体的 deliverable (= 全て実装済み、 §12):

### Phase 2b: WebTransport adapter
- `src/transport/web_transport.ts` — WebTransport wrapper (`WebTransportClient`) と Connection lifecycle (`WebTransportConnection`)
- `src/transport/types.ts` — Transport 抽象化 interface

### Phase 2c: Channel wrapper
- `src/channel/unison_channel.ts` — stream channel
- `src/channel/datagram_channel.ts` — datagram channel
- `src/channel/dispatcher.ts` — datagram dispatcher (= varint demux TS port)

### Phase 2d: Codec
- `src/codec/codec.ts` — Codec interface
- `src/codec/json_codec.ts` — JsonCodec
- `src/codec/proto_codec.ts` — ProtoCodec (= @bufbuild/protobuf wrapping)

### Phase 2e: Tests + bundle
- vitest。 unit は層ごとの directory (`tests/{channel,codec,error,transport,wire}/`)、 結合は `tests/integration/`
- vite production build
- bundle size CI check は未導入 (§6.3)

---

## 10. v0.10.0 Rust 側 API との比較 (= polyglot consistency check)

| 機能 | Rust (= v0.10.0) | TS (= package 1.2.0) |
|---|---|---|
| Connect | `client.connect(url)` | `connect({ url, trust })` |
| Connection events | `client.subscribe_connection_events()` | `client.events()` (AsyncIterable) |
| Stream channel open | `client.open_channel("name")` | `client.openChannel(ChannelMeta)` |
| Datagram channel open | `client.open_datagram_channel("name", id)` | `client.openDatagramChannel(ChannelMeta)` |
| Request | `chan.request::<Req, Resp>("Name", &req)` | `chan.request("Name", payload)` |
| Event subscribe | `chan.recv_event::<T>()` (= loop) | `for await (e of chan.events())` |
| Event send | `chan.send_event(&event)` | `chan.sendEvent("Name", payload)` |
| Disconnect | `client.disconnect()` | `client.disconnect()` |

### 設計上の対称性 / 非対称性

- **対称**: connect / disconnect / connection event の購読 (Rust `subscribe_connection_events()` ↔ TS `events()`)、 概念 1:1
- **対称的だが ergonomic 差**: Rust は turbofish 型 (`request::<Req, Resp>`)、 TS は ChannelMeta + name argument で narrowing。 TS の方が ergonomic 高い (= as const literal narrow による)
- **非対称**: Rust の `open_datagram_channel(name, channel_id)` は 2 引数、 TS の `openDatagramChannel(meta)` は 1 引数 (= meta が channel_id を含む)。 TS の方が「IDE 補完で channel_id 思い出さなくて良い」 ergonomic 優位
- **非対称**: Rust の event subscribe は `loop { recv_event().await }`、 TS は `for await` — どちらも各言語の idiom

---

## 11. Open questions (= Phase 2 implementation 中に解決)

1. **エラー型表現**: Phase 5 の ErrorCategory を TS でどう port? Discriminated union / enum / class hierarchy?
2. **Reconnect helper**: caller 任せだが、 「standard reconnect helper」 (= exponential backoff + jitter) を ergonomic utility として SDK に同梱? v1.x 候補。
3. **Multi-channel single-handler**: 「全 datagram channel を 1 個の callback で受ける」 use case の API? Phase 2c で `client.onAllDatagrams(callback)` 検討。
4. **Schema versioning**: server v1 / client v0 のような version skew 検出 mechanism? Phase 5 (error code) / Phase 6 (docs) で議論。

---

## 12. Phase 2 着手前の checklist

- [x] design doc (= 本 file) commit、 caller code aspiration が固まる
- [x] Phase 2a: `clients/typescript/` skeleton 作成
- [x] Phase 2b: WebTransport adapter
- [x] Phase 2c: Channel wrapper
- [x] Phase 2d: Codec
- [x] Phase 2e: Tests + bundle (bundle size の CI gate は未導入、 §6.3)

いずれも 2026-05-16 に実装 (`git log -- clients/typescript`)。 現在の package は 1.2.0。

各 sub-phase 完了で「caller の TS code が segment ごとに動く」 incremental verification。

---

## 13. 関連

- spec/02-unified-channel/SPEC.md (= unison protocol spec)
- design/datagram-channel.md (= datagram channel mental model)
- design/wire-format.md (= buffa wire format)
- memory: v1.0.0 sprint master (= mem_1Cb4hage4hYjSbqLgbwk2T)
- memory: caller adoption sequencing (= mem_1Cb4gSZmxmHJxxoe8oXPrg)
- claude-plugin-vantage-point repo (= VP dashboard caller context)
