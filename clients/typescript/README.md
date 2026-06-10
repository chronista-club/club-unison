# @chronista-club/unison-client

> TypeScript client SDK for the [Unison protocol](https://github.com/chronista-club/club-unison).
> Part of the **v1.0 polyglot client base** (= server stays Rust, client polyglot for adoption surface).

**Status**: `1.1.0` — v1.0 GA + `/testing` subpath. Protocol API is frozen and the SDK is shipped to npm. It talks to the Rust server over **real WebTransport** (verified by `tests/integration/webtransport_e2e.test.ts`). See [design/typescript-client-api.md](../../design/typescript-client-api.md) for the SDK design contract.

---

## What this is

Unison is a KDL schema-driven, QUIC-based protocol with both **stream channels** (request/response + event) and **datagram channels** (broadcast event). The Rust crate `club-unison` ships the server and Rust client. This package is the **TypeScript client SDK** that lets:

- **Browsers** (via [WebTransport](https://www.w3.org/TR/webtransport/)) speak unison directly to a Rust server (= no REST gateway)
- **Node.js** clients connect server-to-server in TypeScript (= optional, v1.x)

The asymmetric design — **fat Rust server, thin polyglot clients** — keeps complexity (TLS, accept loops, connection management) in one language while broadening the adoption surface.

## v1.0 status (= what shipped)

The v1.0 sprint is feature-complete. All phases are done:

- **Phase 1** ✅ — KDL-driven TypeScript code generation (type interfaces + channel metadata)
- **Phase 2a-e** ✅ — Package skeleton, WebTransport transport adapter, channel wrappers
  (`UnisonChannel<M>` / `DatagramChannel<M>`), codecs (`JsonCodec` + `ProtoCodec`), tests + bundle build
- **Phase 3** ✅ — `connect()` facade + Vantage Point dashboard proof point demo
- **Phase 4-5** ✅ — `unison` developer CLI (ping / sniff / mock / schema-lint), `ErrorCategory` framework
- **Phase 6** ✅ — Rust-compatible wire format + **real WebTransport E2E** (TS SDK ↔ Rust server)
- **Phase 7** ✅ — Cross-language E2E integration tests in CI
- **Phase 8** ✅ — User-facing docs (this file + the guides below)

What is **v1.x deferred** (honest gaps):

- TypeScript codegen for datagram channels (Rust codegen has it; TS handwrites the meta for now)
- proto-descriptor codegen (`ProtoCodec` works, but KDL → descriptor generation is not automated)
- Node native WebTransport (Node needs the `@fails-components/webtransport` polyfill)
- Safari / Firefox WebTransport (Chromium-based browsers are the official support matrix)
- per-channel codec override, auto-reconnect helper

See [`design/typescript-client-api.md`](../../design/typescript-client-api.md) for the full API design contract.

## Quickstart

For the full end-to-end walkthrough (KDL schema → Rust server → TS client), see
[`guides/quickstart.md`](../../guides/quickstart.md). API reference:
[`guides/typescript-sdk.md`](../../guides/typescript-sdk.md).

```typescript
import { connect, type ChannelMeta } from "@chronista-club/unison-client";

const EchoMeta = {
  name: "echo",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: [],
  requests: { Echo: { request: "EchoReq", response: "EchoResp" } },
} as const satisfies ChannelMeta;

// Connect — cert pinning for a dev self-signed server (loopback only).
const client = await connect({
  url: "https://127.0.0.1:4439",
  trust: { certHash: "<CERT_HASH printed by the server>" },
  awaitIdentity: false,
});

const echo = await client.openChannel(EchoMeta);
const reply = await echo.request("Echo", { text: "hello-unison" });
console.log(reply); // { text: "hello-unison" }

await echo.close();
await client.disconnect();
```

## Testing utilities (`/testing` subpath)

Drive the SDK end-to-end without a real WebTransport stack or a Rust server —
`MockTransport` pairs in-memory endpoints and `StreamServerStub` speaks the same
byte-compatible frame protocol as the Rust server:

```typescript
import { connect } from "@chronista-club/unison-client";
import {
  MockTransport,
  StreamServerStub,
} from "@chronista-club/unison-client/testing";

const transport = new MockTransport();
const { server } = transport.prepare(); // server-side endpoint of the memory pipe

const client = await connect({
  url: "https://mock.local", // any URL — never dialed with a mock transport
  transport,
  awaitIdentity: false,
});

// Server side: accept the stream and answer with an echo handler.
const serverSide = (async () => {
  const accepted = await server.acceptStream();
  if (accepted.done) throw new Error("no stream");
  return new StreamServerStub(accepted.value, (_method, payload) => payload);
})();
```

`examples/vp-dashboard.ts` and `tests/integration/beta_freeze.test.ts` show the
full pattern (datagram channels, identity handshake via `sendIdentity`, nack
injection with `rejectOpen`).

The SDK's own integration tests (`tests/integration/`) import the same module,
so the published harness can never drift from what CI verifies.

## Architecture

```
clients/typescript/
├── src/
│   ├── index.ts           ← public entry (= re-exports the surface below)
│   ├── client.ts          ← connect() + UnisonClient facade
│   ├── transport/         ← WebTransport adapter
│   ├── channel/           ← UnisonChannel / DatagramChannel + dispatcher + frame
│   ├── codec/             ← JsonCodec + ProtoCodec
│   ├── wire/              ← Rust-compatible packet / protocol-message encode/decode
│   ├── error/             ← ErrorCategory framework
│   └── testing/           ← /testing subpath (MockTransport + StreamServerStub)
├── examples/              ← vp-dashboard.ts (Vantage Point proof point demo)
├── tests/                 ← vitest unit + integration tests (incl. real WebTransport E2E)
├── package.json
├── tsconfig.json
├── vite.config.ts
└── vitest.config.ts
```

## Building from source

```bash
cd clients/typescript
npm install            # install dev deps (typescript / vite / vitest)
npm run build          # vite build → dist/index.js + tsc → dist/index.d.ts
npm test               # vitest run
npm run typecheck      # tsc --noEmit (= type safety verification)
```

## Versioning policy

- TS package version is kept in **major.minor sync** with the Rust crate `club-unison`
- `1.1.0` (current) — adds the `/testing` subpath + channel util re-exports; protocol API remains frozen since `1.0.0`; breaking changes to the public surface require v2.0
- `1.x.0` — additive features (channels, codecs, error codes) that preserve API compatibility
- `1.0.x` — patch fixes

## Compatibility

| Component | Required |
|---|---|
| Browser | Chromium-based 95+ (= WebTransport native) |
| Node.js | 20+ (= ESM + modern features) |
| TypeScript | 5.7+ (= consumer's tsconfig.json target) |
| Rust server | `club-unison` major.minor 一致 |

Safari / Firefox WebTransport support: tracked in v1.x roadmap, polyfill via WebSocket fallback is deferred to caller demand.

## License

MIT — see [LICENSE](./LICENSE) (bundled) or [LICENSE](../../LICENSE) in the repository root.

## Contributing

This SDK is part of the `chronista-club/club-unison` monorepo. Issues + PRs at https://github.com/chronista-club/club-unison.
