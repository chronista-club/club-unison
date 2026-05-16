/**
 * Stream channel wrapper (= Phase 2c、 Phase 6b で Rust wire 互換に再構築)。
 *
 * QUIC / WebTransport bidi stream 上の request/response + server-pushed event。
 * 内部で 1 本の recv loop を持ち、 受信 `ProtocolMessage` を `msgType` で振り分ける:
 * - `response` / `error` → `id` 対応の pending request を resolve/reject
 * - `event` / `request` → events() の AsyncIterable queue に流す
 *
 * Rust `network/channel.rs` の `UnisonChannel` に対応する TS port。 wire は
 * `frame.ts` の typed frame (= `[4B len][0x00][UnisonPacket]`) で Rust server と
 * byte 一致する。
 *
 * ## channel open
 *
 * stream open 直後に `ProtocolMessage { method: "__channel:{name}",
 * msgType: request }` を 1 本送る (= Rust `client.rs::open_channel` と同形)。
 * Phase 6c 以降、 Rust server は同 stream へ `open_ack` (= method
 * `__channel_ack`、 open request と同 id) を返す。 `waitAccepted` はこの ack を
 * await し、 Response なら resolve / Error (= channel-not-found) なら reject /
 * timeout なら reject する (= optimistic-resolve を廃止)。
 */

import type { Codec } from "../codec/codec.js";
import type { BidiStream } from "../transport/types.js";
import { defaultCodec } from "./default_codec.js";
import { AsyncQueue } from "./async_queue.js";
import {
  type DecodedFrame,
  decodeTypedFrame,
  encodeProtocolFrame,
  readFrames,
} from "./frame.js";
import {
  MSG_TYPE_ERROR,
  MSG_TYPE_EVENT,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
  type ProtocolMessage,
} from "../wire/protocol_message.js";
import type {
  ChannelMeta,
  ChannelPayload,
  EventName,
  EventPayload,
  EventType,
  RequestName,
  RequestType,
  ResponseType,
  UnisonChannel,
} from "./types.js";

/** request() のデフォルト timeout (= Rust 側と同じ 30 秒) */
const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;

/** channel open handshake のデフォルト timeout (= 後方互換のため引数に残す) */
export const DEFAULT_OPEN_TIMEOUT_MS = 5_000;

/** `__channel:` route prefix (= Rust `client.rs::open_channel`) */
const CHANNEL_ROUTE_PREFIX = "__channel:";

/** open_ack の method 名 (= Rust `quic.rs::CHANNEL_ACK_METHOD`、 Phase 6c) */
const CHANNEL_ACK_METHOD = "__channel_ack";

/** 応答待ち request 1 件の resolver ペア */
interface PendingRequest {
  resolve(payload: ChannelPayload): void;
  reject(error: Error): void;
}

/**
 * `UnisonChannel` の concrete impl。 `openChannel(meta)` から構築する
 * (= caller は直接 new せず factory 経由)。
 */
export class UnisonChannelImpl<M extends ChannelMeta>
  implements UnisonChannel<M>
{
  readonly name: M["name"];

  readonly #stream: BidiStream;
  readonly #codec: Codec<ChannelPayload>;
  readonly #writer: WritableStreamDefaultWriter<Uint8Array>;
  /** id → 応答待ち request */
  readonly #pending = new Map<number, PendingRequest>();
  /** server push event の queue (= events() が配る) */
  readonly #events = new AsyncQueue<ChannelPayload>();
  /** recv loop の完了 promise */
  readonly #recvLoop: Promise<void>;
  /** open frame 送信前に stream 終端したら set される */
  #recvEnded = false;
  #nextId = 1;
  #closed = false;

  /** @internal `openChannel` から呼ぶ。 */
  constructor(
    meta: M,
    stream: BidiStream,
    codec: Codec<ChannelPayload> = defaultCodec,
  ) {
    this.name = meta.name;
    this.#stream = stream;
    this.#codec = codec;
    this.#writer = stream.writable.getWriter();
    this.#recvLoop = this.#runRecvLoop();
  }

  /**
   * @internal `openChannel` から呼ぶ。 `__channel:{name}` open frame を送り、
   * server の `open_ack` を await する (= Phase 6c、 real accept signal)。
   *
   * server は同 stream へ open request と同 id の `__channel_ack` frame を返す:
   * - Response → accept、 resolve する
   * - Error → nack (= channel-not-found 等)、 reject する
   *
   * `timeoutMs` 内に ack が来なければ reject する。 send 自体が失敗 / accept
   * 前に stream が終端した場合も no-accept として reject する。
   */
  async waitAccepted(timeoutMs: number = DEFAULT_OPEN_TIMEOUT_MS): Promise<void> {
    const id = this.#nextId++;
    const openMsg: ProtocolMessage = {
      id,
      method: `${CHANNEL_ROUTE_PREFIX}${this.name}`,
      msgType: MSG_TYPE_REQUEST,
      payload: this.#codec.encode({}),
    };
    // open_ack は recv loop が #pending 経由で resolve/reject する。
    // open frame を書く前に pending を登録する (= ack が先着しても取りこぼさない)。
    const ack = new Promise<ChannelPayload>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
    });
    try {
      await this.#writer.write(encodeProtocolFrame(openMsg));
    } catch (cause) {
      this.#pending.delete(id);
      throw new Error(
        `channel "${this.name}" could not be opened ` +
          `(= failed to write the __channel open frame)`,
        { cause },
      );
    }
    // open frame は流せたが recv loop が既に終端しているなら peer は居ない
    if (this.#recvEnded) {
      this.#pending.delete(id);
      throw new Error(
        `channel "${this.name}" closed before it was accepted ` +
          `(= no server peer accepted the bidi stream)`,
      );
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(
          new Error(
            `channel "${this.name}" was not accepted within ${timeoutMs}ms ` +
              `(= no open_ack from the server peer)`,
          ),
        );
      }, timeoutMs);
    });
    try {
      await Promise.race([ack, timeout]);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  /** 受信 frame を msgType で振り分ける background loop */
  async #runRecvLoop(): Promise<void> {
    try {
      for await (const body of readFrames(this.#stream.readable)) {
        let decoded: DecodedFrame;
        try {
          decoded = decodeTypedFrame(body);
        } catch {
          continue; // malformed frame は drop
        }
        if (decoded.type === "raw") {
          // raw frame は stream channel では未使用 — drop
          continue;
        }
        this.#dispatch(decoded.message);
      }
    } catch {
      // stream error は terminate 扱い
    } finally {
      this.#recvEnded = true;
      this.#failAllPending("channel closed");
      this.#events.end();
    }
  }

  /** 1 個の `ProtocolMessage` を pending / events へ振り分ける */
  #dispatch(message: ProtocolMessage): void {
    if (
      message.msgType === MSG_TYPE_RESPONSE ||
      message.msgType === MSG_TYPE_ERROR
    ) {
      const pending = this.#pending.get(message.id);
      if (pending === undefined) return;
      this.#pending.delete(message.id);
      if (message.msgType === MSG_TYPE_ERROR) {
        pending.reject(new Error(this.#errorText(message.payload)));
      } else {
        this.#tryResolve(pending, message.payload);
      }
      return;
    }
    // event / request → events queue
    this.#tryPushEvent(message.payload);
  }

  #tryResolve(pending: PendingRequest, payload: Uint8Array): void {
    try {
      pending.resolve(this.#codec.decode(payload));
    } catch (cause) {
      pending.reject(cause instanceof Error ? cause : new Error(String(cause)));
    }
  }

  #tryPushEvent(payload: Uint8Array): void {
    try {
      this.#events.push(this.#codec.decode(payload));
    } catch {
      // decode 不能 event は drop
    }
  }

  #errorText(payload: Uint8Array): string {
    try {
      return `channel "${this.name}" request error: ${JSON.stringify(this.#codec.decode(payload))}`;
    } catch {
      return `channel "${this.name}" request error`;
    }
  }

  #failAllPending(reason: string): void {
    for (const pending of this.#pending.values()) {
      pending.reject(new Error(reason));
    }
    this.#pending.clear();
  }

  async request<N extends RequestName<M>>(
    name: N,
    payload: RequestType<M, N>,
  ): Promise<ResponseType<M, N>> {
    if (this.#closed) throw new Error(`channel "${this.name}" is closed`);
    const id = this.#nextId++;
    const frame = encodeProtocolFrame({
      id,
      method: name,
      msgType: MSG_TYPE_REQUEST,
      payload: this.#codec.encode(payload as ChannelPayload),
    });
    const result = new Promise<ChannelPayload>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
    });
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`request "${name}" timed out`));
      }, DEFAULT_REQUEST_TIMEOUT_MS);
    });
    try {
      await this.#writer.write(frame);
      return (await Promise.race([result, timeout])) as ResponseType<M, N>;
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  events(): AsyncIterableIterator<EventType<M>> {
    return this.#events as AsyncIterableIterator<EventType<M>>;
  }

  async sendEvent<N extends EventName<M>>(
    name: N,
    payload: EventPayload<M, N>,
  ): Promise<void> {
    if (this.#closed) throw new Error(`channel "${this.name}" is closed`);
    await this.#writer.write(
      encodeProtocolFrame({
        id: 0,
        method: name,
        msgType: MSG_TYPE_EVENT,
        payload: this.#codec.encode(payload as ChannelPayload),
      }),
    );
  }

  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    try {
      this.#writer.releaseLock();
    } catch {
      // already released
    }
    await this.#stream.close();
    await this.#recvLoop;
  }
}
