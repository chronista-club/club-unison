/**
 * WebTransport adapter (= Phase 2b 実装)。
 *
 * browser native WebTransport を `Transport` / `Connection` 抽象に bridge する。
 * trust mode / cert pinning は `./trust.ts`、 error 型は `./errors.ts` に分離。
 */

import { WebTransportUnsupportedError } from "./errors.js";
import { buildWebTransportOptions } from "./trust.js";
import type {
  BidiStream,
  Connection,
  ConnectionEvent,
  ConnectOptions,
  Transport,
  TrustMode,
} from "./types.js";

/** skip-verify (= cert pinning) を許可する loopback host */
const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "::1"]);

/**
 * cert pinning は loopback host にのみ許可する hard gate。
 * 非 loopback への pinning は本番想定の誤用なので throw。
 *
 * @internal export は単体テスト用 (= WebTransport 不在の node 環境では
 * `connect()` 経由でゲートに到達できないため、 gate を直接検証する)。
 */
export function enforceTrustGate(
  url: string,
  trust: TrustMode | undefined,
): void {
  if (trust === undefined || trust === "system") return;
  let host: string;
  try {
    host = new URL(url).hostname;
  } catch {
    throw new Error(`invalid connection URL: ${url}`);
  }
  // URL.hostname は IPv6 を角括弧付きで返す (= `[::1]`) が LOOPBACK_HOSTS は
  // 括弧なし (= `::1`) で保持するため、 比較前に外側の角括弧を剥がして正規化。
  const normalizedHost = host.replace(/^\[|\]$/g, "");
  if (!LOOPBACK_HOSTS.has(normalizedHost)) {
    throw new Error(
      `cert pinning (skip-verify) is restricted to localhost; got host "${host}"`,
    );
  }
}

/** WebTransport bidi stream を `BidiStream` に wrap */
function wrapBidiStream(stream: WebTransportBidirectionalStream): BidiStream {
  return {
    readable: stream.readable,
    writable: stream.writable,
    async close(): Promise<void> {
      await Promise.allSettled([
        stream.readable.cancel(),
        stream.writable.close(),
      ]);
    },
  };
}

/** WebTransport-backed Connection の concrete impl */
export class WebTransportConnection implements Connection {
  readonly #url: string;
  readonly #transport: WebTransport;
  /** events() の購読者へ流す sink 群 (= 終端 event 待ち) */
  readonly #eventSinks = new Set<(e: ConnectionEvent) => void>();
  /** 確定した終端 event (= 後発購読者へ即時 replay) */
  #terminal: ConnectionEvent | undefined;
  /** server-opened bidi stream を 1 本ずつ取り出す reader (= lazy) */
  #incomingReader:
    | ReadableStreamDefaultReader<WebTransportBidirectionalStream>
    | undefined;

  /** @internal `connect()` から呼ぶ。 caller は factory を使う。 */
  constructor(url: string, transport: WebTransport) {
    this.#url = url;
    this.#transport = transport;
    // .closed の解決を connection event に翻訳 (= Q4: 単一 event source)
    transport.closed
      .then(() => this.#emit({ type: "disconnected", reason: "closed" }))
      .catch((err: unknown) => {
        this.#emit({
          type: "error",
          error: err instanceof Error ? err : new Error(String(err)),
        });
      });
  }

  /** 接続先 URL (= debug / log 用に expose) */
  get url(): string {
    return this.#url;
  }

  #emit(event: ConnectionEvent): void {
    if (this.#terminal !== undefined) return; // 終端は 1 回のみ
    this.#terminal = event;
    for (const sink of this.#eventSinks) sink(event);
    this.#eventSinks.clear();
  }

  async openBidiStream(): Promise<BidiStream> {
    const stream = await this.#transport.createBidirectionalStream();
    return wrapBidiStream(stream);
  }

  async acceptBidiStream(): Promise<BidiStream | undefined> {
    if (this.#incomingReader === undefined) {
      this.#incomingReader =
        this.#transport.incomingBidirectionalStreams.getReader();
    }
    const { value, done } = await this.#incomingReader.read();
    if (done || value === undefined) return undefined;
    return wrapBidiStream(value);
  }

  async sendDatagram(payload: Uint8Array): Promise<void> {
    const datagrams = this.#transport.datagrams;
    const max = datagrams.maxDatagramSize;
    if (payload.byteLength > max) {
      throw new Error(
        `datagram of ${payload.byteLength} bytes exceeds max size ${max}`,
      );
    }
    const writer = datagrams.writable.getWriter();
    try {
      await writer.write(payload);
    } finally {
      writer.releaseLock();
    }
  }

  async *datagrams(): AsyncIterableIterator<Uint8Array> {
    const reader = this.#transport.datagrams.readable.getReader();
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        if (value !== undefined) yield value;
      }
    } finally {
      reader.releaseLock();
    }
  }

  async *events(): AsyncIterableIterator<ConnectionEvent> {
    // 終端済みなら connected を出さず終端 event のみ replay (= 正しい現状反映)
    if (this.#terminal !== undefined) {
      yield this.#terminal;
      return;
    }
    // 接続中: connected を 1 件、 続いて終端 event 1 件を流す
    yield { type: "connected", remoteAddr: this.#url };
    const event = await new Promise<ConnectionEvent>((resolve) => {
      this.#eventSinks.add(resolve);
    });
    yield event;
  }

  async close(reason?: string): Promise<void> {
    if (this.#terminal !== undefined) return;
    this.#transport.close(reason !== undefined ? { reason } : undefined);
    this.#emit({ type: "disconnected", reason: reason ?? "closed by client" });
  }
}

/** WebTransport `Transport` 実装 (= SDK default) */
export class WebTransportClient implements Transport {
  connect(opts: ConnectOptions): Promise<Connection> {
    return connect(opts.url, opts);
  }
}

/**
 * WebTransport 接続を確立し `Connection` を返す。
 *
 * - WebTransport 非対応環境では `WebTransportUnsupportedError`。
 * - cert pinning は loopback host に限定 (= 非 loopback で throw)。
 * - `opts.signal` は connection 全体の kill-switch — abort で `connect()` 中断、
 *   確立後は connection + 配下 stream を tear down する。
 */
export async function connect(
  url: string,
  opts: ConnectOptions = { url },
): Promise<Connection> {
  if (typeof WebTransport === "undefined") {
    throw new WebTransportUnsupportedError();
  }
  const { signal } = opts;
  signal?.throwIfAborted();

  enforceTrustGate(url, opts.trust);

  const transport = new WebTransport(url, buildWebTransportOptions(opts.trust));

  // signal abort → transport 強制 close (= 配下 stream も全て tear down)
  const onAbort = (): void => transport.close();
  signal?.addEventListener("abort", onAbort, { once: true });

  try {
    await transport.ready;
  } catch (err) {
    signal?.removeEventListener("abort", onAbort);
    if (signal?.aborted === true) signal.throwIfAborted();
    throw err instanceof Error
      ? err
      : new Error(`WebTransport connect failed: ${String(err)}`);
  }

  // ready 解決と abort が race した場合に備え、 abort 済みなら即 throw
  if (signal?.aborted === true) {
    transport.close();
    signal.throwIfAborted();
  }

  return new WebTransportConnection(url, transport);
}
