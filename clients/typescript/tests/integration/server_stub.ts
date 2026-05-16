/**
 * Server-side frame echo stub (= Phase 2e、 Phase 6b で Rust wire 互換に再構築)。
 *
 * mock bidi stream の server 端を drain し、 受信 `ProtocolMessage` typed frame を
 * Rust server と同じ規則で処理する:
 * - `__channel:{name}` (= request) → channel open probe、 そのまま継続
 * - `request` → handler を呼んで `response` frame を返す
 * - `event` → `receivedEvents` に記録
 *
 * frame layout は `src/channel/frame.ts` の typed frame (= `[4B len][0x00]
 * [UnisonPacket]`) で Rust `quic.rs` と byte 一致する。
 */

import type { BidiStream, Connection } from "../../src/transport/types.js";
import {
  decodeTypedFrame,
  encodeProtocolFrame,
  readFrames,
} from "../../src/channel/frame.js";
import {
  MSG_TYPE_ERROR,
  MSG_TYPE_EVENT,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
} from "../../src/wire/protocol_message.js";
import { JsonCodec } from "../../src/codec/json_codec.js";
import type { ServerIdentity } from "../../src/channel/identity.js";

const codec = JsonCodec.shared;
const textEncoder = new TextEncoder();

/** `__channel:` route prefix (= Rust `client.rs::open_channel`) */
const CHANNEL_ROUTE_PREFIX = "__channel:";

/** request → response payload を決める handler */
export type RequestHandler = (
  method: string,
  payload: Record<string, unknown>,
) => Record<string, unknown>;

/** server 側で受信した event frame */
export interface ReceivedEvent {
  method: string;
  payload: Record<string, unknown>;
}

/** 1 本の bidi stream を server として運転する stub */
export class StreamServerStub {
  readonly #writer: WritableStreamDefaultWriter<Uint8Array>;
  readonly receivedEvents: ReceivedEvent[] = [];
  /** open probe で観測した channel 名 (= `__channel:` を剥がした後) */
  openedChannel: string | undefined;
  readonly #loop: Promise<void>;

  constructor(
    private readonly stream: BidiStream,
    private readonly handler: RequestHandler = () => ({ ok: true }),
  ) {
    this.#writer = stream.writable.getWriter();
    this.#loop = this.#run();
  }

  async #run(): Promise<void> {
    try {
      for await (const body of readFrames(this.stream.readable)) {
        const decoded = decodeTypedFrame(body);
        if (decoded.type !== "protocol") continue; // raw frame は無視
        const msg = decoded.message;

        // channel open probe (= `__channel:{name}` request) — 記録して継続
        if (msg.method.startsWith(CHANNEL_ROUTE_PREFIX)) {
          this.openedChannel = msg.method.slice(CHANNEL_ROUTE_PREFIX.length);
          continue;
        }

        const payload = decodePayload(msg.payload);
        if (msg.msgType === MSG_TYPE_REQUEST) {
          const resp = this.handler(msg.method, payload);
          await this.#writer.write(
            encodeProtocolFrame({
              id: msg.id,
              method: msg.method,
              msgType: MSG_TYPE_RESPONSE,
              payload: codec.encode(resp),
            }),
          );
        } else if (msg.msgType === MSG_TYPE_EVENT) {
          this.receivedEvents.push({ method: msg.method, payload });
        }
      }
    } catch {
      /* stream closed */
    }
  }

  /** client へ server-push event を送る */
  async pushEvent(
    method: string,
    payload: Record<string, unknown>,
  ): Promise<void> {
    await this.#writer.write(
      encodeProtocolFrame({
        id: 0,
        method,
        msgType: MSG_TYPE_EVENT,
        payload: codec.encode(payload),
      }),
    );
  }

  /** client へ error response を送る (= 指定 request id 宛て) */
  async pushError(
    id: number,
    method: string,
    payload: Record<string, unknown>,
  ): Promise<void> {
    await this.#writer.write(
      encodeProtocolFrame({
        id,
        method,
        msgType: MSG_TYPE_ERROR,
        payload: codec.encode(payload),
      }),
    );
  }

  async close(): Promise<void> {
    try {
      this.#writer.releaseLock();
    } catch {
      /* released */
    }
    await this.stream.close();
    await this.#loop;
  }
}

/** payload bytes を JSON object として decode (= 空なら `{}`) */
function decodePayload(payload: Uint8Array): Record<string, unknown> {
  if (payload.length === 0) return {};
  return codec.decode(payload) as Record<string, unknown>;
}

/** test 用デフォルト identity (= Rust `ServerIdentity` 形状) */
export const TEST_IDENTITY: ServerIdentity = {
  name: "test-server",
  version: "1.0.0",
  namespace: "club.chronista.test",
  channels: [
    {
      name: "control",
      direction: "bidirectional",
      lifetime: "persistent",
      status: "available",
    },
  ],
  metadata: null,
};

/**
 * server 側 connection から identity stream を 1 本 open し、 `__identity`
 * frame を送って finish する (= Rust `handle_connection` の identity 送出)。
 */
export async function sendIdentity(
  serverConn: Connection,
  identity: ServerIdentity = TEST_IDENTITY,
): Promise<void> {
  const stream = await serverConn.openBidiStream();
  const writer = stream.writable.getWriter();
  try {
    await writer.write(
      encodeProtocolFrame({
        id: 0,
        method: "__identity",
        msgType: MSG_TYPE_EVENT,
        payload: textEncoder.encode(JSON.stringify(identity)),
      }),
    );
  } finally {
    writer.releaseLock();
  }
  await stream.close();
}
