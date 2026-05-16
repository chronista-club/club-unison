/**
 * 実 WebTransport E2E (= Phase 6d Step 2)。
 *
 * **mock を使わない初の実ラウンドトリップ。** 実際の TS SDK が、 実 WebTransport
 * 経由で、 起動した Rust unison server に接続する。
 *
 * 構成:
 * 1. Rust echo server (`cargo run --example webtransport_echo_server`) を子プロセス
 *    として spawn し、 stdout の `CERT_HASH=` / `READY ` 行を待つ。
 * 2. `@fails-components/webtransport` の `WebTransport` を `globalThis` へ polyfill
 *    (= Node には native WebTransport が無いため)。 これで SDK の transport が動く。
 * 3. SDK の `connect()` を、 server の cert hash を `trust` に pin して呼ぶ。
 * 4. `echo` channel で request ラウンドトリップ、 `clock` channel で event 受信。
 * 5. assert → tear down。
 *
 * native quiche binding が load できない環境では test 全体を skip する
 * (= CI / 環境差での偽 red を避ける、 honest signal)。
 */

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { connect } from "../../src/index.js";
import type { ChannelMeta } from "../../src/channel/types.js";

/** repo root (= clients/typescript から 2 つ上) */
const REPO_ROOT = resolve(import.meta.dirname, "../../../..");

/** echo server の起動を待つ最大時間 (= cargo の cold build を考慮) */
const SERVER_BOOT_TIMEOUT_MS = 180_000;

// ── polyfill の load を試みる ────────────────────────────────
// native quiche binding を含むため、 load 失敗時は test を skip する。
let polyfillWebTransport: typeof WebTransport | undefined;
let polyfillError: string | undefined;
try {
  const mod = await import("@fails-components/webtransport");
  await mod.quicheLoaded;
  polyfillWebTransport = mod.WebTransport as unknown as typeof WebTransport;
} catch (err) {
  polyfillError = err instanceof Error ? err.message : String(err);
}

/** echo channel の meta (= server の `register_channel("echo", ...)` に対応) */
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

/** clock channel の meta (= server の `register_channel("clock", ...)` に対応) */
const ClockMeta = {
  name: "clock",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: ["Tick"],
  requests: {},
} as const satisfies ChannelMeta;

/** spawn した echo server の制御ハンドル */
interface ServerHandle {
  readonly proc: ChildProcessWithoutNullStreams;
  readonly url: string;
  readonly certHash: string;
}

/**
 * Rust echo server を spawn し、 stdout 契約 (`CERT_HASH=` / `READY `) を待つ。
 */
function spawnEchoServer(addr: string): Promise<ServerHandle> {
  return new Promise<ServerHandle>((resolvePromise, rejectPromise) => {
    const proc = spawn(
      "cargo",
      [
        "run",
        "--quiet",
        "-p",
        "club-unison",
        "--example",
        "webtransport_echo_server",
        "--",
        addr,
      ],
      { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "pipe"] },
    );

    let certHash: string | undefined;
    let url: string | undefined;
    let stdoutBuf = "";
    let stderrBuf = "";

    const timer = setTimeout(() => {
      proc.kill("SIGKILL");
      rejectPromise(
        new Error(
          `echo server がタイムアウト (${SERVER_BOOT_TIMEOUT_MS}ms)\n` +
            `stderr:\n${stderrBuf}`,
        ),
      );
    }, SERVER_BOOT_TIMEOUT_MS);

    proc.stdout.setEncoding("utf8");
    proc.stdout.on("data", (chunk: string) => {
      stdoutBuf += chunk;
      for (const line of stdoutBuf.split("\n")) {
        if (line.startsWith("CERT_HASH=")) {
          certHash = line.slice("CERT_HASH=".length).trim();
        } else if (line.startsWith("READY ")) {
          const m = /addr=(\S+)/.exec(line);
          if (m) url = m[1];
        }
      }
      if (certHash !== undefined && url !== undefined) {
        clearTimeout(timer);
        resolvePromise({ proc, url, certHash });
      }
    });

    proc.stderr.setEncoding("utf8");
    proc.stderr.on("data", (chunk: string) => {
      stderrBuf += chunk;
    });

    proc.on("error", (err) => {
      clearTimeout(timer);
      rejectPromise(err);
    });
    proc.on("exit", (code) => {
      if (certHash === undefined || url === undefined) {
        clearTimeout(timer);
        rejectPromise(
          new Error(`echo server が起動前に exit (code ${code})\n${stderrBuf}`),
        );
      }
    });
  });
}

const suite = polyfillWebTransport !== undefined ? describe : describe.skip;

suite("WebTransport E2E (TS SDK ↔ Rust server)", () => {
  let server: ServerHandle;
  let priorWebTransport: typeof WebTransport | undefined;

  beforeAll(async () => {
    if (polyfillError !== undefined) {
      console.warn(`[webtransport_e2e] polyfill 読み込み失敗: ${polyfillError}`);
    }
    // SDK transport が参照する global を polyfill へ差し替える。
    priorWebTransport = (globalThis as { WebTransport?: typeof WebTransport })
      .WebTransport;
    (globalThis as { WebTransport?: typeof WebTransport }).WebTransport =
      polyfillWebTransport;
    server = await spawnEchoServer("127.0.0.1:4439");
  }, SERVER_BOOT_TIMEOUT_MS + 5_000);

  afterAll(() => {
    server?.proc.kill("SIGKILL");
    (globalThis as { WebTransport?: typeof WebTransport }).WebTransport =
      priorWebTransport;
  });

  it("echo channel: request ラウンドトリップが実 WebTransport で成立する", async () => {
    const client = await connect({
      url: server.url,
      trust: { certHash: server.certHash },
      awaitIdentity: false,
    });
    try {
      const echo = await client.openChannel(EchoMeta);
      const reply = await echo.request("Echo", { text: "hello-unison" });
      expect(reply).toEqual({ text: "hello-unison" });
      await echo.close();
    } finally {
      await client.disconnect();
    }
  }, 30_000);

  it("clock channel: server-pushed event を実 WebTransport で受信する", async () => {
    const client = await connect({
      url: server.url,
      trust: { certHash: server.certHash },
      awaitIdentity: false,
    });
    try {
      const clock = await client.openChannel(ClockMeta);
      const received: unknown[] = [];
      for await (const ev of clock.events()) {
        received.push(ev);
        if (received.length === 3) break;
      }
      expect(received).toEqual([{ seq: 0 }, { seq: 1 }, { seq: 2 }]);
      await clock.close();
    } finally {
      await client.disconnect();
    }
  }, 30_000);
});
