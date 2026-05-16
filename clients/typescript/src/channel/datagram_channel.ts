/**
 * Datagram channel wrapper (= Phase 2c)。
 *
 * 共有 datagram path 上の virtual stream。 受信は `DatagramDispatcher` が
 * `channelId` で demux した payload を pull、 送信は `channelId` varint prefix +
 * codec-encoded payload を組み立てて `Connection.sendDatagram` に渡す。
 *
 * Rust `network/datagram_channel.rs` の `DatagramChannel` に対応する TS port。
 */

import type { Codec } from "../codec/codec.js";
import type { Connection } from "../transport/types.js";
import { defaultCodec } from "./default_codec.js";
import { AsyncQueue } from "./async_queue.js";
import type { DatagramDispatcher, DatagramSink } from "./dispatcher.js";
import type {
  ChannelPayload,
  DatagramChannel,
  DatagramChannelMeta,
  EventName,
  EventPayload,
  EventType,
} from "./types.js";
import { encodeVarint } from "./varint.js";

/**
 * `DatagramChannel` の concrete impl。 `openDatagramChannel(meta)` から構築する
 * (= caller は直接 new せず factory 経由)。
 */
export class DatagramChannelImpl<M extends DatagramChannelMeta>
  implements DatagramChannel<M>
{
  readonly name: M["name"];
  readonly channelId: M["channelId"];

  readonly #connection: Connection;
  readonly #dispatcher: DatagramDispatcher;
  readonly #codec: Codec<ChannelPayload>;
  /** demux 後 payload を AsyncIterable に橋渡しする queue (= 単一消費者) */
  readonly #queue = new AsyncQueue<ChannelPayload>();
  /** `channelId` prefix の事前計算 varint (= 送信 hot path 用) */
  readonly #idPrefix: Uint8Array;
  #closed = false;

  /** @internal `openDatagramChannel` から呼ぶ。 */
  constructor(
    meta: M,
    connection: Connection,
    dispatcher: DatagramDispatcher,
    codec: Codec<ChannelPayload> = defaultCodec,
  ) {
    this.name = meta.name;
    this.channelId = meta.channelId;
    this.#connection = connection;
    this.#dispatcher = dispatcher;
    this.#codec = codec;
    this.#idPrefix = encodeVarint(meta.channelId);

    // dispatcher に sink を登録 — demux 済 payload を decode して queue に流す
    const sink: DatagramSink = {
      push: (payload: Uint8Array): void => {
        try {
          this.#queue.push(this.#codec.decode(payload));
        } catch {
          // decode 失敗は drop (= unreliable semantics、 caller に伝えない)
        }
      },
      end: (): void => this.#queue.end(),
    };
    dispatcher.register(meta.channelId, sink);
  }

  events(): AsyncIterableIterator<EventType<M>> {
    return this.#queue as AsyncIterableIterator<EventType<M>>;
  }

  async sendEvent<N extends EventName<M>>(
    _name: N,
    payload: EventPayload<M, N>,
  ): Promise<void> {
    if (this.#closed) throw new Error(`datagram channel "${this.name}" is closed`);
    const encoded = this.#codec.encode(payload as ChannelPayload);
    // [varint channelId] [codec-encoded payload] を組み立て
    const buf = new Uint8Array(this.#idPrefix.length + encoded.length);
    buf.set(this.#idPrefix, 0);
    buf.set(encoded, this.#idPrefix.length);
    await this.#connection.sendDatagram(buf);
  }

  close(): Promise<void> {
    if (this.#closed) return Promise.resolve();
    this.#closed = true;
    this.#dispatcher.unregister(this.channelId); // unregister が sink.end → queue 終端
    return Promise.resolve();
  }
}
