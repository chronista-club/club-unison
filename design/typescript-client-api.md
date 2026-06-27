# design/typescript-client-api.md — TS Client SDK API Design (= v1.0 Phase 3a deliverable)

**バージョン**: 0.1-draft (= v1.0.0-alpha 系列で iterative refine)
**最終更新**: 2026-05-16
**ステータス**: Draft — Phase 2 (TS runtime SDK) implementation の API contract、 demo-driven design

---

## 1. 目的

「**ideal caller TS code を先に sketch して、 Phase 2 SDK の API surface を逆設計する**」 ための design exercise。 v0.10.0 で Rust 側で取った top-down approach (= codegen 出力 1f → 実装 1d/1e) を polyglot で再現。

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
            field "value" type="number" required=#true
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

## 4. SDK API surface 要件 (= Phase 2 implementation contract)

### 4.1 Top-level API

```typescript
export interface UnisonClientConnectOptions {
  url: string;
  trust?: "system" | "skip-verify" | { cert: string };
  reconnect?: never;  // library does NOT auto-reconnect (= caller 責務)
}

export const unisonClient = {
  async connect(opts: UnisonClientConnectOptions): Promise<UnisonClient>;
};

export interface UnisonClient {
  serverIdentity(): Promise<ServerIdentity>;
  subscribeConnectionEvents(): ConnectionEventEmitter;
  // openChannel は open handshake (= `open` frame → server `open_ack`) を行い、
  // server peer が bidi stream を accept したことを確認してから resolve する。
  // `openTimeoutMs` 内に accept されなければ reject + stream tear down (= no-accept signal)。
  openChannel<M extends ChannelMeta>(meta: M, openTimeoutMs?: number): Promise<UnisonChannel<M>>;
  openDatagramChannel<M extends DatagramChannelMeta>(meta: M): DatagramChannel<M>;
  disconnect(): Promise<void>;
}
```

caller-facing の connection entry は **free function `connect`** で export する
(= `import { connect } from "@chronista-club/unison-client"`)。 transport-level の
低レベル connect は `connectTransport` として別 export、 facade と名前衝突しない。

### 4.2 Channel API

```typescript
// Stream channel
export interface UnisonChannel<M extends ChannelMeta> {
  // Request → Response (= meta.requests[N].request / response 型推論)
  request<N extends keyof M["requests"]>(
    name: N,
    payload: RequestType<M, N>,
  ): Promise<ResponseType<M, N>>;

  // Event subscribe (= server push、 AsyncIterable)
  events(): AsyncIterable<EventType<M>>;

  // Event send (= client → server、 from が client または either の時)
  sendEvent<N extends EventName<M>>(name: N, payload: EventPayload<M, N>): Promise<void>;

  close(): Promise<void>;
}

// Datagram channel (= event only、 request 不可)
export interface DatagramChannel<M extends DatagramChannelMeta> {
  events(): AsyncIterable<EventType<M>>;
  sendEvent<N extends EventName<M>>(name: N, payload: EventPayload<M, N>): Promise<void>;
  channelId: number;  // = compile-time const (from meta)
  close(): Promise<void>;
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

// Phase 2 type narrowing:
type EventName<M> = M extends { events: readonly (infer N)[] } ? N : never;
type EventType<M> = // __types.events 経由で生成 interface に解決
  M extends { __types?: infer T } ? T["events"][keyof T["events"]] : ChannelPayload;

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
export interface ConnectionEventEmitter {
  on(event: "connected", handler: (e: { remoteAddr: string }) => void): void;
  on(event: "disconnected", handler: (e: { reason: string }) => void): void;
  // EventTarget standard interface 互換 (= browser ergonomic)
}
```

### 4.5 Auth API (connection-level、 v1.4.0)

connection-level auth primitive（Rust `enable_auth` / `connect_with_credential`、設計
`design/connection-auth.md`）の TS 対応。auth は `unison.auth` channel を open して
`Authenticate` request を送るだけなので、専用 transport は不要。

```typescript
// connect-time auth (= Rust connect_with_credential の対応物)
export interface UnisonConnectOptions {
  // ...既存...
  /** 接続直後に 1 回提示する credential。指定すると identity handshake 後に
   *  unison.auth で Authenticate し、拒否 (ok=false) なら connect が throw する。 */
  credential?: Uint8Array;
}

export interface UnisonClient {
  // ...既存...
  /** post-connect / re-auth 用。unison.auth を open して Authenticate を送り、
   *  ok=false なら throw。connect({credential}) は内部でこれを呼ぶ。 */
  authenticate(credential: Uint8Array): Promise<void>;
}
```

**wire 不変条件**（`design/connection-auth.md` §5.8 が SSOT）:
- request payload = `{ credential: number[] }`。`Uint8Array` は **`Array.from(bytes)` で `number[]`
  に変換**してから送る（直接入れると JSON が `{"0":..}` object 化して Rust の `Vec<u8>` と非互換）。
- response = `{ ok: boolean }`。`ok === false` → `authenticate` / `connect` が throw。

reserved channel meta（`AUTH_CHANNEL` const）と定数（`AUTH_CHANNEL_NAME` = `"unison.auth"` /
`AUTHENTICATE_METHOD` = `"Authenticate"`）は SDK が内蔵し、codegen 不要。

---

## 5. Codec 戦略

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

- `import { unisonClient } from "@chronista-club/unison-client"` 時、 caller が使わない codec / transport adapter は dead-code elim
- Sub-path import 検討: `import { unisonClient } from "@chronista-club/unison-client/transport"` で transport だけ
- Phase 2e で size check CI gate (= bundle が threshold 超えたら fail)

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
const client = await unisonClient.connect({ url: "..." });
// 内部で `typeof WebTransport === "undefined"` check
// 不可なら明示 error: "WebTransport not supported, use Chromium-based browser"
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

design doc が確定したら Phase 2b/c/d/e の具体的 deliverable:

### Phase 2b: WebTransport adapter
- `src/transport/web_transport.ts` — WebTransport wrapper
- `src/transport/connection.ts` — Connection lifecycle
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
- `tests/unit/` — vitest unit tests
- `tests/integration/` — Phase 7 で本格化
- vite production build
- bundle size CI check

---

## 10. v0.10.0 Rust 側 API との比較 (= polyglot consistency check)

| 機能 | Rust (= v0.10.0) | TS (= v1.0 提案) |
|---|---|---|
| Connect | `client.connect(url)` | `unisonClient.connect({ url, trust })` |
| Connection events | `client.subscribe_connection_events()` | `client.subscribeConnectionEvents()` |
| Stream channel open | `client.open_channel("name")` | `client.openChannel(ChannelMeta)` |
| Datagram channel open | `client.open_datagram_channel("name", id)` | `client.openDatagramChannel(ChannelMeta)` |
| Request | `chan.request::<Req, Resp>("Name", &req)` | `chan.request("Name", payload)` |
| Event subscribe | `chan.recv_event::<T>()` (= loop) | `for await (e of chan.events())` |
| Event send | `chan.send_event(&event)` | `chan.sendEvent("Name", payload)` |
| Disconnect | `client.disconnect()` | `client.disconnect()` |

### 設計上の対称性 / 非対称性

- **対称**: connect / disconnect / subscribe_connection_events、 概念 1:1
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
- [ ] Phase 2a: `clients/typescript/` skeleton 作成 (= 次の session 着手候補)
- [ ] Phase 2b: WebTransport adapter (= 別 session)
- [ ] Phase 2c: Channel wrapper (= 別 session)
- [ ] Phase 2d: Codec (= 別 session)
- [ ] Phase 2e: Tests + bundle (= 別 session)

各 sub-phase 完了で「caller の TS code が segment ごとに動く」 incremental verification。

---

## 13. 関連

- spec/02-unified-channel/SPEC.md (= unison protocol spec)
- design/datagram-channel.md (= datagram channel mental model)
- design/wire-format.md (= buffa wire format)
- memory: v1.0.0 sprint master (= mem_1Cb4hage4hYjSbqLgbwk2T)
- memory: caller adoption sequencing (= mem_1Cb4gSZmxmHJxxoe8oXPrg)
- claude-plugin-vantage-point repo (= VP dashboard caller context)
