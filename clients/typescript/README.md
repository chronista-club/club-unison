# @chronista-club/unison-client

> TypeScript client SDK for the [Unison protocol](https://github.com/chronista-club/club-unison).
> Part of the **v1.0 polyglot client base** (= server stays Rust, client polyglot for adoption surface).

**Status**: `1.0.0-alpha.1` — early development. API not yet stable, see [design/typescript-client-api.md](../../design/typescript-client-api.md) for the SDK design contract.

---

## What this is

Unison is a KDL schema-driven, QUIC-based protocol with both **stream channels** (request/response + event) and **datagram channels** (broadcast event). The Rust crate `club-unison` ships the server and Rust client. This package is the **TypeScript client SDK** that lets:

- **Browsers** (via [WebTransport](https://www.w3.org/TR/webtransport/)) speak unison directly to a Rust server (= no REST gateway)
- **Node.js** clients connect server-to-server in TypeScript (= optional, v1.x)

The asymmetric design — **fat Rust server, thin polyglot clients** — keeps complexity (TLS, accept loops, connection management) in one language while broadening the adoption surface.

## v1.0 roadmap (= what's coming)

This SDK is being built in phases:

- **Phase 1** ✅ — KDL-driven TypeScript code generation (= type interfaces + channel metadata)
- **Phase 2a** ✅ — Package skeleton (= this file's commit point)
- **Phase 2b** — WebTransport transport adapter
- **Phase 2c** — Channel wrappers (`UnisonChannel<M>` / `DatagramChannel<M>` TS port)
- **Phase 2d** — Codecs (`JsonCodec` + `ProtoCodec` via `@bufbuild/protobuf`)
- **Phase 2e** — Tests + bundle build (= ≤ 200 KB minified gzipped target)
- **Phase 3b** — First proof point demo (= Vantage Point dashboard subscribe)
- **Phase 4-7** — CLI tools, error code framework, docs, CI integration tests
- **v1.0.0-rc.1** — Dogfood phase across chronista-club ecosystem
- **v1.0.0** — Stability commitment (= dogfood exit criteria: 3+ caller × 3+ months × critical bug 0)

See [`design/typescript-client-api.md`](../../design/typescript-client-api.md) for the full API design contract.

## Quickstart (= will be filled in as Phase 2b/c/d/e land)

```typescript
// ❗ This API is aspirational — Phase 2 implementation in progress.
// See design doc for the contract that this code will satisfy.

import { unisonClient } from "@chronista-club/unison-client";
import { MetricChannelMeta, type MetricUpdate } from "./generated/vp-protocol";

const client = await unisonClient.connect({
  url: "https://vp.chronista.local:8080",
  trust: "system",
});

const metricChan = await client.openDatagramChannel(MetricChannelMeta);

for await (const update of metricChan.events()) {
  // update is typed as MetricUpdate (= via ChannelMeta type narrowing)
  dashboardStore.set(update.name, update.value);
}
```

## Architecture (= preview)

```
clients/typescript/
├── src/
│   ├── index.ts           ← public entry (= Phase 2a 雛形、 Phase 2b- で fill)
│   ├── transport/         ← WebTransport adapter (= Phase 2b)
│   ├── channel/           ← Channel wrappers (= Phase 2c)
│   └── codec/             ← JsonCodec + ProtoCodec (= Phase 2d)
├── tests/                 ← vitest unit + integration tests (= Phase 2e)
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
- `1.0.0-alpha.x` — Phase 2 implementation in progress, breaking changes allowed
- `1.0.0-beta.x` — Phase 2 complete, API freeze candidates, refinement only
- `1.0.0-rc.x` — Feature complete, dogfood phase with chronista-club ecosystem
- `1.0.0` — Stability commitment, breaking changes require v2.0

## Compatibility

| Component | Required |
|---|---|
| Browser | Chromium-based 95+ (= WebTransport native) |
| Node.js | 20+ (= ESM + modern features) |
| TypeScript | 5.7+ (= consumer's tsconfig.json target) |
| Rust server | `club-unison` major.minor 一致 |

Safari / Firefox WebTransport support: tracked in v1.x roadmap, polyfill via WebSocket fallback is deferred to caller demand.

## License

MIT — see [LICENSE](../../LICENSE-MIT) in the repository root.

## Contributing

This SDK is part of the `chronista-club/club-unison` monorepo. Issues + PRs at https://github.com/chronista-club/club-unison.
