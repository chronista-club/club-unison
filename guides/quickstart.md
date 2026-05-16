# クイックスタート — Unison サーバ + TypeScript クライアント

KDL スキーマで定義した channel を、Rust サーバと TypeScript クライアントで
end-to-end に疎通させるまでの手順。**ここに載るコード・コマンドは v1.0 時点で
実際に動作するもの**（`clients/typescript/tests/integration/webtransport_e2e.test.ts`
が実 WebTransport で検証している構成）。

---

## 1. Unison とは

Unison は **KDL スキーマ駆動の QUIC プロトコルフレームワーク**。

- **サーバ・プロトコルコア**: Rust crate `club-unison`（lib 名 `unison`）
- **開発者 CLI**: `unison`（crate `unison-cli`）— ping / sniff / mock / schema-lint
- **TypeScript クライアント SDK**: `@chronista-club/unison-client`
  — ブラウザ / Node.js から WebTransport 経由でサーバに直結

設計思想は **「太い Rust サーバ + 細い polyglot クライアント」**。
TLS・accept loop・接続管理の複雑さを Rust 側に閉じ込め、クライアントは
各言語の薄い SDK にする。

通信は全て **channel** を経由する（旧 RPC は廃止済み）:

| channel 種別 | backend | 用途 |
|---|---|---|
| stream channel | `stream`（default） | request/response + event push、順序保証あり |
| datagram channel | `datagram` | broadcast event のみ、unreliable / unordered |

---

## 2. KDL スキーマを書く

channel 定義を KDL で書く。例として `echo`（request/response）と
`clock`（event push）の 2 channel を持つスキーマ:

```kdl
// schemas/quickstart.kdl
protocol "quickstart" version="1.0.0" {
    namespace "com.example.quickstart"

    // request/response channel
    channel "echo" from="client" lifetime="persistent" {
        request "Echo" {
            field "text" type="string" required=#true
            returns "EchoResp" {
                field "text" type="string"
            }
        }
    }

    // server push 専用 channel
    channel "clock" from="client" lifetime="persistent" {
        event "Tick" {
            field "seq" type="number"
        }
    }
}
```

主要属性:

| 属性 | 値 | 意味 |
|---|---|---|
| `from` | `"server"` / `"client"` / `"either"` | どちらが送信を開始するか |
| `lifetime` | `"persistent"` / `"transient"` | 接続中維持 / リクエスト単位で開閉 |
| `backend` | `"stream"`（default）/ `"datagram"` | トランスポート種別 |
| `channel_id` | 正整数 | `backend="datagram"` 時のみ必須 |

スキーマの妥当性は CLI で検証できる:

```bash
!cargo run -p unison-cli -- schema-lint schemas/quickstart.kdl
```

`schema-lint` は KDL syntax error に加え、datagram の `channel_id` 衝突 /
channel 名重複 / request・event 名の重複などの不変条件もチェックする。

---

## 3. Rust サーバを起動する

最短経路は同梱の echo サーバ example。これは上記スキーマの `echo` / `clock`
channel を実装し、WebTransport ingress を開く:

```bash
!cargo run -p club-unison --example webtransport_echo_server -- 127.0.0.1:4439
```

起動すると stdout に 2 行を印字する（クライアントが parse する契約）:

```text
CERT_HASH=<64 hex chars>
READY addr=https://127.0.0.1:4439
```

- `CERT_HASH` は **leaf 証明書の SHA-256 hex hash**。自己署名証明書なので、
  クライアントはこの値で証明書を pin する（後述）。
- log は stderr に流れる（`RUST_LOG` で制御）。

サーバ側のハンドラ実装（`webtransport_echo_server.rs` 抜粋）:

```rust
use unison::ProtocolServer;
use unison::network::webtransport::WebTransportServer;
use unison::network::{MessageType, UnisonChannel};
use unison::network::quic::UnisonStream;

let server = ProtocolServer::with_identity(
    "unison-webtransport-echo", env!("CARGO_PKG_VERSION"), "example",
);

// echo channel: request の `text` をそのまま返す
server.register_channel("echo", |_ctx, stream| async move {
    let channel: UnisonChannel = UnisonChannel::new(stream);
    loop {
        match channel.recv().await {
            Ok(msg) if msg.msg_type == MessageType::Request => {
                let req: serde_json::Value =
                    serde_json::from_slice(&msg.payload).unwrap_or_default();
                let reply = serde_json::json!({ "text": req.get("text") });
                channel.send_response(msg.id, &msg.method, &reply).await?;
            }
            Ok(_) => {}
            Err(e) if e.is_normal_close() => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}).await;

let mut wt = WebTransportServer::dev(Arc::new(server));
wt.bind("127.0.0.1:4439".parse()?).await?;
wt.start().await?;
```

> KDL 駆動の素の QUIC サーバ（WebTransport なし）の書き方は
> [`channel-guide.md`](channel-guide.md) を参照。

---

## 4. TypeScript クライアントから接続する

### 4.1 channel meta を用意する

クライアントは channel ごとに `ChannelMeta` を渡す。これは KDL スキーマから
codegen される（後述）が、手書きしても良い。`echo` channel の meta:

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

### 4.2 接続して request を投げる

```typescript
import { connect } from "@chronista-club/unison-client";

// サーバが印字した CERT_HASH を trust に pin する。
// 自己署名証明書なので "system" ではなく cert pinning を使う。
const client = await connect({
  url: "https://127.0.0.1:4439",
  trust: { certHash: "<CERT_HASH from server stdout>" },
  awaitIdentity: false,
});

const echo = await client.openChannel(EchoMeta);
const reply = await echo.request("Echo", { text: "hello-unison" });
console.log(reply); // { text: "hello-unison" }

await echo.close();
await client.disconnect();
```

### 4.3 server push event を受信する

`clock` channel は `Tick` event を 200ms 間隔で push する。event は
`AsyncIterable` として読む:

```typescript
const ClockMeta = {
  name: "clock", backend: "stream", from: "client", lifetime: "persistent",
  events: ["Tick"], requests: {},
} as const satisfies ChannelMeta;

const clock = await client.openChannel(ClockMeta);
for await (const ev of clock.events()) {
  console.log(ev); // { seq: 0 }, { seq: 1 }, ...
  // break すると iterator が閉じ、channel close へ cascade する
}
```

---

## 5. ブラウザと cert-hash pinning

### 5.1 trust モード

`connect()` の `trust` には 2 つのモードがある:

| `trust` 値 | 用途 |
|---|---|
| `"system"`（default） | システム CA で検証する公開サーバ |
| `{ certHash: "<64 hex>" }` | 自己署名証明書を SHA-256 で pin する dev サーバ |

ブラウザの WebTransport には「全 cert 検証を skip する」モードは**存在しない**。
自己署名証明書を使う場合は必ず cert-hash pinning（`serverCertificateHashes`）
を使う。これが Rust サーバが起動時に `CERT_HASH` を印字する理由。

### 5.2 cert pinning は loopback 限定

SDK は cert pinning を **loopback host（`localhost` / `127.0.0.1` / `[::1]`）
にのみ許可する** hard gate を持つ。非 loopback への pinning は本番想定の
誤用なので例外を投げる。本番の公開サーバは `trust: "system"` + CA 署名証明書を
使うこと。

### 5.3 ブラウザ要件

| 環境 | WebTransport |
|---|---|
| Chromium 系（Chrome / Edge / Opera）95+ | native 対応、production |
| Safari 18+ | datagram は部分対応、stream は実装中 |
| Firefox | flag 有効化が必要 |

v1.0 では **Chromium 系のみ公式サポート**。Safari / Firefox 対応と
WebSocket polyfill は v1.x で caller demand 駆動。

### 5.4 Node.js から接続する

Node には native WebTransport が無いため、`@fails-components/webtransport`
を `globalThis.WebTransport` に polyfill して使う（E2E test がこの方式）。
Node native QUIC adapter は v1.x deferred。

---

## 6. CLI で疎通を確認する

サーバなしで開発を進める / 素早く疎通を見るための CLI:

```bash
# KDL schema を検証する
!cargo run -p unison-cli -- schema-lint schemas/quickstart.kdl

# KDL schema から stub server を起動する（実バックエンド不要）
!cargo run -p unison-cli -- mock --schema schemas/quickstart.kdl --addr '[::1]:7878'

# サーバへ疎通 + RTT 計測
!cargo run -p unison-cli -- ping 'quic://[::1]:7878'

# channel traffic を覗く packet inspector
!cargo run -p unison-cli -- sniff 'quic://[::1]:7878' --channel echo
```

`mock` は KDL の `returns` 型から決定的に stub payload を組み立てて返す
（string→`""`, number→`0`, bool→`false`, json→`{}`）。

---

## 7. 次に読むもの

- [migration.md](migration.md) — v1.0 までの破壊的変更
- [typescript-sdk.md](typescript-sdk.md) — TS SDK API リファレンス
- [channel-guide.md](channel-guide.md) — Rust 側 channel API
- [spec/02-unified-channel/SPEC.md](../spec/02-unified-channel/SPEC.md) — プロトコル仕様

---

**最終更新**: 2026-05-17（v1.0）
