/**
 * Datagram dispatcher (= Phase 2c) — Rust `network/datagram_dispatcher.rs` の TS port。
 *
 * `Connection.datagrams()` は単一の exclusive reader iterator しか払い出せない。
 * dispatcher はその唯一の iterator を **占有** し、 受信 datagram 先頭の varint
 * `channel_id` を decode、 登録済 channel handler に fan-out する。
 *
 * 配送特性 (= datagram semantics):
 * - **Unreliable**: malformed varint / 未登録 channel_id / buffer 溢れは silently drop。
 * - **Unordered**: 到着順 deliver、 sequence 保証なし。
 * - **Best-effort**: dispatcher loop 自身は決して詰まらない (= 各 handler に try-push)。
 */

import type { Connection } from "../transport/types.js";
import { decodeVarint } from "./varint.js";

/** 1 channel 分の受信 sink (= dispatcher が demux 後の payload を push) */
export interface DatagramSink {
  /** demux 済 payload を push (= best-effort、 buffer 溢れは sink 側で drop) */
  push(payload: Uint8Array): void;
  /** channel close 時に dispatcher が呼ぶ (= pending iterator を終端) */
  end(): void;
}

/**
 * Datagram dispatch table の data 層 (= `quinn` 抜きで test 可能、 Rust の
 * `DispatcherInner` 相当)。 純粋な `channel_id → sink` map 管理に責務限定。
 */
export class DispatcherInner {
  readonly #handlers = new Map<number, DatagramSink>();

  /** `channelId` に sink を登録 (= 既存は replace、 旧 sink は end) */
  register(channelId: number, sink: DatagramSink): void {
    this.#handlers.get(channelId)?.end();
    this.#handlers.set(channelId, sink);
  }

  /** `channelId` の登録解除 */
  unregister(channelId: number): void {
    const sink = this.#handlers.get(channelId);
    if (sink === undefined) return;
    this.#handlers.delete(channelId);
    sink.end();
  }

  /** 登録 channel 数 (= test / debug 用) */
  get handlerCount(): number {
    return this.#handlers.size;
  }

  /**
   * 1 datagram を dispatch (= varint decode → handler lookup → push)。
   * malformed / 未登録 は silently drop (= unreliable semantics)。
   */
  dispatch(datagram: Uint8Array): void {
    const parsed = decodeVarint(datagram);
    if (parsed === null) return; // malformed channel_id varint
    const sink = this.#handlers.get(parsed.value);
    if (sink === undefined) return; // 未登録 channel_id
    sink.push(datagram.subarray(parsed.consumed));
  }

  /** 全 handler を end して clear (= connection 終端時) */
  clear(): void {
    for (const sink of this.#handlers.values()) sink.end();
    this.#handlers.clear();
  }
}

/**
 * Per-connection datagram dispatcher の runtime 層 (= Rust `DatagramDispatcher` 相当)。
 *
 * `Connection.datagrams()` の単一 iterator を background loop で drain し、
 * `DispatcherInner` に流す。 `register` / `unregister` で channel handler を出し入れ、
 * `stop()` で loop 停止 + 全 handler clear。
 */
export class DatagramDispatcher {
  readonly #inner = new DispatcherInner();
  readonly #connection: Connection;
  #loop: Promise<void> | undefined;
  #stopped = false;

  constructor(connection: Connection) {
    this.#connection = connection;
  }

  /** background drain loop を起動 (= idempotent、 初回 `register` で呼ぶ) */
  start(): void {
    if (this.#loop !== undefined || this.#stopped) return;
    this.#loop = this.#drain();
  }

  async #drain(): Promise<void> {
    try {
      for await (const datagram of this.#connection.datagrams()) {
        if (this.#stopped) break;
        this.#inner.dispatch(datagram);
      }
    } catch {
      // connection 断 / reader error は dispatch 終端、 caller には伝えない
    } finally {
      this.#inner.clear();
    }
  }

  /** `channelId` に sink を登録 (= 初回で drain loop を起動) */
  register(channelId: number, sink: DatagramSink): void {
    this.start();
    this.#inner.register(channelId, sink);
  }

  /** `channelId` の登録解除 */
  unregister(channelId: number): void {
    this.#inner.unregister(channelId);
  }

  /** 登録 channel 数 (= test / debug 用) */
  get handlerCount(): number {
    return this.#inner.handlerCount;
  }

  /** dispatcher を停止 (= drain loop 終了 + 全 handler clear) */
  stop(): void {
    this.#stopped = true;
    this.#inner.clear();
  }
}
