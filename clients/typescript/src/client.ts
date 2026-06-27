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
import {
  AUTH_CHANNEL_META,
  AUTHENTICATE_METHOD,
  toAuthenticateRequest,
} from "./channel/auth.js";
import { defaultCodec } from "./channel/default_codec.js";
import { DatagramChannelImpl } from "./channel/datagram_channel.js";
import { DatagramDispatcher } from "./channel/dispatcher.js";
import {
  DEFAULT_IDENTITY_TIMEOUT_MS,
  performIdentityHandshake,
  type ServerIdentity,
} from "./channel/identity.js";
import type {
  ChannelMeta,
  ChannelPayload,
  DatagramChannel,
  DatagramChannelMeta,
  UnisonChannel,
} from "./channel/types.js";
import {
  DEFAULT_OPEN_TIMEOUT_MS,
  UnisonChannelImpl,
} from "./channel/unison_channel.js";
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
  /**
   * identity handshake を待つか (= default: true)。 Unison server は接続直後に
   * identity stream を 1 本送る。 `false` にすると connect は handshake を待たず
   * 即 resolve する (= identity 未対応 server / 高速接続向け)。
   */
  awaitIdentity?: boolean;
  /** identity handshake の timeout ms (= default: 5000) */
  identityTimeoutMs?: number;
  /**
   * 接続直後に 1 回提示する credential (= connection-level auth、 v1.4.0)。指定すると
   * identity handshake 後に `unison.auth` で Authenticate し、 拒否 (ok=false) なら
   * connect が throw する。server が `enable_auth` 未設定なら open が reject される。
   * 設計: `design/connection-auth.md` §5.8。
   */
  credential?: Uint8Array;
}

/**
 * 確立済み connection を束ねる SDK facade。 `connect()` で生成する
 * (= caller は直接 new せず factory 経由)。
 */
export class UnisonClient {
  readonly #connection: Connection;
  readonly #dispatcher: DatagramDispatcher;
  readonly #codec: Codec<ChannelPayload>;
  readonly #identity: ServerIdentity | undefined;
  #closed = false;

  /** @internal `connect()` から呼ぶ。 */
  constructor(
    connection: Connection,
    codec: Codec<ChannelPayload> = defaultCodec,
    identity?: ServerIdentity,
  ) {
    this.#connection = connection;
    this.#dispatcher = new DatagramDispatcher(connection);
    this.#codec = codec;
    this.#identity = identity;
  }

  /**
   * connect 時に受信した server identity (= 自己紹介)。
   *
   * `awaitIdentity: false` で接続した / handshake が来なかった場合は
   * `undefined`。
   */
  serverIdentity(): ServerIdentity | undefined {
    return this.#identity;
  }

  /** Connection lifecycle event の購読 (= connected / disconnected / error) */
  events() {
    return this.#connection.events();
  }

  /**
   * Stream channel を開設 (= bidi stream を 1 本 open、 request/response + event)。
   *
   * open handshake (= `open` frame → server `open_ack`) を行い、 server peer が
   * stream を accept したことを確認してから resolve する。 `openTimeoutMs` 内に
   * accept されなければ reject + stream を tear down する (= no-accept signal)。
   */
  async openChannel<M extends ChannelMeta>(
    meta: M,
    openTimeoutMs: number = DEFAULT_OPEN_TIMEOUT_MS,
  ): Promise<UnisonChannel<M>> {
    if (this.#closed) throw new Error("client is closed");
    const stream = await this.#connection.openBidiStream();
    const channel = new UnisonChannelImpl(meta, stream, this.#codec);
    try {
      await channel.waitAccepted(openTimeoutMs);
    } catch (cause) {
      await channel.close().catch(() => undefined);
      throw cause;
    }
    return channel;
  }

  /** Datagram channel を開設 (= 共有 datagram path、 broadcast event のみ) */
  openDatagramChannel<M extends DatagramChannelMeta>(meta: M): DatagramChannel<M> {
    if (this.#closed) throw new Error("client is closed");
    return new DatagramChannelImpl(meta, this.#connection, this.#dispatcher, this.#codec);
  }

  /**
   * credential を提示して connection を認証する (= connection-level authN、 v1.4.0)。
   *
   * `unison.auth` channel を open して `Authenticate` request を 1 回送り、 server の
   * verifier 結果を待つ。`ok=false` (= 拒否) なら throw。認証成功後は server 側が
   * この connection の principal を立てるので、 以降 open する channel は per-message
   * gate を通過できる。**他 channel を open する前に呼ぶこと** (= 未認証だと principal
   * None で app handler に gate される)。
   *
   * server が `enable_auth` 未設定なら `unison.auth` が未登録で open が reject される。
   * `connect({ credential })` は内部でこれを呼ぶ。設計: `design/connection-auth.md` §5.8。
   */
  async authenticate(credential: Uint8Array): Promise<void> {
    if (this.#closed) throw new Error("client is closed");
    const channel = await this.openChannel(AUTH_CHANNEL_META);
    try {
      // response は ChannelPayload (= Record<string, unknown>)。ok!==true を拒否扱い。
      const result = await channel.request(
        AUTHENTICATE_METHOD,
        toAuthenticateRequest(credential),
      );
      if (result["ok"] !== true) {
        throw new Error("authentication denied by server verifier");
      }
    } finally {
      await channel.close().catch(() => undefined);
    }
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
  const codec = opts.codec ?? defaultCodec;

  // identity handshake (= server-opened stream の `__identity` を読む)。
  // default で待つが、 失敗しても connection 自体は使えるので best-effort。
  let identity: ServerIdentity | undefined;
  if (opts.awaitIdentity !== false) {
    try {
      identity = await performIdentityHandshake(
        connection,
        opts.identityTimeoutMs ?? DEFAULT_IDENTITY_TIMEOUT_MS,
      );
    } catch {
      // identity を返さない server / timeout — connection は維持し identity なし
      identity = undefined;
    }
  }
  const client = new UnisonClient(connection, codec, identity);

  // connection-level auth (= opts.credential 指定時、 identity handshake 後に 1 回認証)。
  // 拒否なら connection を畳んで throw する (= 認証必須を caller に伝える)。
  if (opts.credential !== undefined) {
    try {
      await client.authenticate(opts.credential);
    } catch (err) {
      await client.disconnect().catch(() => undefined);
      throw err;
    }
  }
  return client;
}
