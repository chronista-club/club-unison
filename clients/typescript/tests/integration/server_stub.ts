/**
 * Server-side frame echo stub (= Phase 2e integration test 用)。
 *
 * mock bidi stream の server 端を drain し、 受信 `request` frame に対して
 * `response` frame を返す。 `pushEvent()` で client へ event frame を送れる。
 */

import type { BidiStream } from "../../src/transport/types.js";
import {
  decodeFrameBody,
  encodeFrame,
  type FrameHeader,
  readFrames,
} from "../../src/channel/frame.js";
import { JsonCodec } from "../../src/codec/json_codec.js";

const codec = JsonCodec.shared;

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
        const { header, payload } = decodeFrameBody(body);
        if (header.type === "open") {
          // channel open probe — accept の証拠として open_ack を返す
          await this.#writer.write(
            encodeFrame(
              { id: 0, method: header.method, type: "open_ack" },
              codec.encode({}),
            ),
          );
          continue;
        }
        const msg = codec.decode(payload) as Record<string, unknown>;
        if (header.type === "request") {
          const respHeader: FrameHeader = {
            id: header.id,
            method: header.method,
            type: "response",
          };
          const resp = this.handler(header.method, msg);
          await this.#writer.write(encodeFrame(respHeader, codec.encode(resp)));
        } else if (header.type === "event") {
          this.receivedEvents.push({ method: header.method, payload: msg });
        }
      }
    } catch {
      /* stream closed */
    }
  }

  /** client へ server-push event を送る */
  async pushEvent(method: string, payload: Record<string, unknown>): Promise<void> {
    await this.#writer.write(
      encodeFrame({ id: 0, method, type: "event" }, codec.encode(payload)),
    );
  }

  /** client へ error response を送る (= 指定 request id 宛て) */
  async pushError(id: number, method: string, payload: Record<string, unknown>): Promise<void> {
    await this.#writer.write(
      encodeFrame({ id, method, type: "error" }, codec.encode(payload)),
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
