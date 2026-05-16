/**
 * In-memory mock transport (= Phase 2e integration test 基盤)。
 *
 * 実 WebTransport なしで channel を E2E 駆動するための `Connection` /
 * `Transport` 実装。 2 endpoint (= client 側 / server 側) を memory pipe で繋ぐ。
 *
 * - `MockConnection.openBidiStream()` → server に paired stream を push
 * - `sendDatagram()` → server 側 datagram queue に enqueue (= 逆も同様)
 * - `events()` → connected を即発火、 `close()` で disconnected を流す
 */

import type {
  BidiStream,
  Connection,
  ConnectionEvent,
  ConnectOptions,
  Transport,
} from "../../src/transport/types.js";
import { AsyncQueue } from "../../src/channel/async_queue.js";

/** 片方向 byte pipe。 `endReadable` で読み手側を即終端できる (= reader cancel 相当) */
interface BytePipe {
  writable: WritableStream<Uint8Array>;
  readable: ReadableStream<Uint8Array>;
  /** readable 側を EOF 終端 (= reader が locked でも安全) */
  endReadable(): void;
}

/**
 * 片方向 byte pipe (= writable 側 → readable 側)。
 *
 * vitest の `node` 環境では global `TransformStream` が deadlock するため、
 * `ReadableStream` controller を直結する自前 pipe で代替する。
 */
function bytePipe(): BytePipe {
  let controller: ReadableStreamDefaultController<Uint8Array>;
  let done = false;
  const finish = (): void => {
    if (done) return;
    done = true;
    try {
      controller.close();
    } catch {
      /* already closed */
    }
  };
  const readable = new ReadableStream<Uint8Array>({
    start: (c) => {
      controller = c;
    },
  });
  const writable = new WritableStream<Uint8Array>({
    write: (chunk) => {
      if (!done) controller.enqueue(chunk);
    },
    close: finish,
    abort: finish,
  });
  return { writable, readable, endReadable: finish };
}

/**
 * 1 本の双方向 stream を 2 endpoint 分 (= client view / server view) 生成。
 *
 * `close()` は real `wrapBidiStream` 同様、 writable を close (= peer に EOF) し、
 * かつ自身の readable を終端する (= reader cancel 相当)。
 */
export function makeBidiPair(): { client: BidiStream; server: BidiStream } {
  const c2s = bytePipe(); // client write → server read
  const s2c = bytePipe(); // server write → client read
  const client: BidiStream = {
    readable: s2c.readable,
    writable: c2s.writable,
    close: async () => {
      try {
        await c2s.writable.close();
      } catch {
        /* already closed */
      }
      s2c.endReadable();
    },
  };
  const server: BidiStream = {
    readable: c2s.readable,
    writable: s2c.writable,
    close: async () => {
      try {
        await s2c.writable.close();
      } catch {
        /* already closed */
      }
      c2s.endReadable();
    },
  };
  return { client, server };
}

/**
 * Memory 上の `Connection` 実装。 `MockConnection.pair()` で client/server 2 個を
 * 同時生成し、 datagram / bidi stream を互いに配送する。
 */
export class MockConnection implements Connection {
  readonly #datagramsIn = new AsyncQueue<Uint8Array>();
  readonly #events = new AsyncQueue<ConnectionEvent>();
  #peer: MockConnection | undefined;
  /** server 側で accept 待ちの stream queue */
  readonly #acceptQueue = new AsyncQueue<BidiStream>();
  /** この endpoint が保持する bidi stream (= connection close で連鎖 tear down) */
  readonly #streams = new Set<BidiStream>();
  #closed = false;

  private constructor(readonly remoteAddr: string) {}

  /** client / server 1 ペアを生成 (= 互いに peer 参照) */
  static pair(): { client: MockConnection; server: MockConnection } {
    const client = new MockConnection("mock://server");
    const server = new MockConnection("mock://client");
    client.#peer = server;
    server.#peer = client;
    client.#events.push({ type: "connected", remoteAddr: client.remoteAddr });
    server.#events.push({ type: "connected", remoteAddr: server.remoteAddr });
    return { client, server };
  }

  async openBidiStream(): Promise<BidiStream> {
    if (this.#closed) throw new Error("connection closed");
    const { client, server } = makeBidiPair();
    // この endpoint が client view、 peer に server view を push
    this.#streams.add(client);
    if (this.#peer) {
      this.#peer.#acceptQueue.push(server);
      this.#peer.#streams.add(server);
    }
    return client;
  }

  /** server 側: peer が open した stream を受け取る (= test helper) */
  acceptStream(): Promise<IteratorResult<BidiStream>> {
    return this.#acceptQueue.next();
  }

  async sendDatagram(payload: Uint8Array): Promise<void> {
    if (this.#closed) throw new Error("connection closed");
    if (this.#peer) this.#peer.#datagramsIn.push(payload.slice());
  }

  datagrams(): AsyncIterable<Uint8Array> {
    return this.#datagramsIn;
  }

  events(): AsyncIterable<ConnectionEvent> {
    return this.#events;
  }

  async close(reason = "closed"): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    this.#events.push({ type: "disconnected", reason });
    this.#events.end();
    this.#datagramsIn.end();
    this.#acceptQueue.end();
    // connection close は配下 bidi stream を全て tear down (= real QUIC 同様)
    for (const s of this.#streams) await s.close().catch(() => undefined);
    this.#streams.clear();
    if (this.#peer && !this.#peer.#closed) await this.#peer.close(reason);
  }
}

/** `MockConnection` を払い出す `Transport` (= `connect()` で client 側を返す) */
export class MockTransport implements Transport {
  #pending: { client: MockConnection; server: MockConnection } | undefined;

  /** 次の `connect()` が払い出す pair を server 側に観測できる形で生成 */
  prepare(): { client: MockConnection; server: MockConnection } {
    this.#pending = MockConnection.pair();
    return this.#pending;
  }

  async connect(_opts: ConnectOptions): Promise<Connection> {
    const pair = this.#pending ?? MockConnection.pair();
    this.#pending = undefined;
    return pair.client;
  }
}
