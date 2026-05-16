/**
 * Top-level SDK facade (= Phase 3b)。
 *
 * `connect()` で `Connection` を 1 個確立し、 配下に `DatagramDispatcher` を 1 個
 * 持つ。 caller は `UnisonClient` の `openChannel` / `openDatagramChannel` から
 * channel を開設するだけでよく、 transport / dispatcher の手配線は不要。
 *
 * design `typescript-client-api.md` §4.1 の `UnisonClient` interface 実装。
 */

import type { Codec } from "./codec/codec.js";
import { defaultCodec } from "./channel/default_codec.js";
import { DatagramChannelImpl } from "./channel/datagram_channel.js";
import { DatagramDispatcher } from "./channel/dispatcher.js";
import type {
  ChannelMeta,
  ChannelPayload,
  DatagramChannel,
  DatagramChannelMeta,
  UnisonChannel,
} from "./channel/types.js";
import { UnisonChannelImpl } from "./channel/unison_channel.js";
import type { Connection, ConnectOptions, Transport } from "./transport/types.js";
import { WebTransportClient } from "./transport/web_transport.js";

/** `connect()` への入力 (= `ConnectOptions` を SDK レベルに拡張) */
export interface UnisonConnectOptions extends ConnectOptions {
  /**
   * 使用する transport (= default: WebTransport)。 test では mock transport を
   * 注入する。 caller は通常省略する。
   */
  transport?: Transport;
  /** 全 channel 共有の payload codec (= default: JsonCodec、 design §5.1) */
  codec?: Codec<ChannelPayload>;
}

/**
 * 確立済み connection を束ねる SDK facade。 `connect()` で生成する
 * (= caller は直接 new せず factory 経由)。
 */
export class UnisonClient {
  readonly #connection: Connection;
  readonly #dispatcher: DatagramDispatcher;
  readonly #codec: Codec<ChannelPayload>;
  #closed = false;

  /** @internal `connect()` から呼ぶ。 */
  constructor(connection: Connection, codec: Codec<ChannelPayload> = defaultCodec) {
    this.#connection = connection;
    this.#dispatcher = new DatagramDispatcher(connection);
    this.#codec = codec;
  }

  /** Connection lifecycle event の購読 (= connected / disconnected / error) */
  events() {
    return this.#connection.events();
  }

  /** Stream channel を開設 (= bidi stream を 1 本 open、 request/response + event) */
  async openChannel<M extends ChannelMeta>(meta: M): Promise<UnisonChannel<M>> {
    if (this.#closed) throw new Error("client is closed");
    const stream = await this.#connection.openBidiStream();
    return new UnisonChannelImpl(meta, stream, this.#codec);
  }

  /** Datagram channel を開設 (= 共有 datagram path、 broadcast event のみ) */
  openDatagramChannel<M extends DatagramChannelMeta>(meta: M): DatagramChannel<M> {
    if (this.#closed) throw new Error("client is closed");
    return new DatagramChannelImpl(meta, this.#connection, this.#dispatcher, this.#codec);
  }

  /** Connection を閉じる (= dispatcher 停止 + 配下 channel を tear down) */
  async disconnect(reason?: string): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    this.#dispatcher.stop();
    await this.#connection.close(reason);
  }
}

/**
 * Unison server に接続し `UnisonClient` を返す。
 *
 * - `opts.transport` 省略時は WebTransport (= browser native)。
 * - library は auto-reconnect しない (= caller 責務、 design §4.1)。
 */
export async function connect(opts: UnisonConnectOptions): Promise<UnisonClient> {
  const transport = opts.transport ?? new WebTransportClient();
  const connection = await transport.connect(opts);
  return new UnisonClient(connection, opts.codec ?? defaultCodec);
}
