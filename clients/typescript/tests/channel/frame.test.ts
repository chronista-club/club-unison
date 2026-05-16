/**
 * typed frame encode/decode unit test (= Phase 6b、 Rust wire 互換)。
 *
 * `frame.ts` の typed frame (= `[4B len][1B type][UnisonPacket]`) round-trip と
 * `readFrames` の chunk 再結合を検証する。
 */

import { describe, expect, it } from "vitest";
import {
  FRAME_TYPE_PROTOCOL,
  FRAME_TYPE_RAW,
  decodeTypedFrame,
  encodeProtocolFrame,
  encodeRawFrame,
  readFrames,
} from "../../src/channel/frame.js";
import {
  MSG_TYPE_REQUEST,
  type ProtocolMessage,
} from "../../src/wire/protocol_message.js";

const payload = new TextEncoder().encode('{"x":1}');
const message: ProtocolMessage = {
  id: 3,
  method: "Ping",
  msgType: MSG_TYPE_REQUEST,
  payload,
};

describe("typed frame encode/decode", () => {
  it("round-trips a ProtocolMessage through a PROTOCOL frame", () => {
    const frame = encodeProtocolFrame(message);
    const body = frame.subarray(4); // 4B length prefix を剥がす
    const decoded = decodeTypedFrame(body);
    expect(decoded.type).toBe("protocol");
    if (decoded.type !== "protocol") throw new Error("unreachable");
    expect(decoded.message.id).toBe(3);
    expect(decoded.message.method).toBe("Ping");
    expect(decoded.message.msgType).toBe(MSG_TYPE_REQUEST);
    expect([...decoded.message.payload]).toEqual([...payload]);
  });

  it("writes a big-endian u32 length prefix covering type tag + payload", () => {
    const frame = encodeProtocolFrame(message);
    const totalLen = new DataView(frame.buffer).getUint32(0, false);
    expect(totalLen).toBe(frame.length - 4);
  });

  it("tags a PROTOCOL frame with 0x00", () => {
    const frame = encodeProtocolFrame(message);
    expect(frame[4]).toBe(FRAME_TYPE_PROTOCOL);
  });

  it("round-trips a RAW frame", () => {
    const raw = Uint8Array.from([1, 2, 3, 4, 5]);
    const frame = encodeRawFrame(raw);
    expect(frame[4]).toBe(FRAME_TYPE_RAW);
    const decoded = decodeTypedFrame(frame.subarray(4));
    expect(decoded.type).toBe("raw");
    if (decoded.type !== "raw") throw new Error("unreachable");
    expect([...decoded.data]).toEqual([...raw]);
  });

  it("rejects an unknown frame type tag", () => {
    expect(() => decodeTypedFrame(Uint8Array.from([0x7f, 0x00]))).toThrow();
  });
});

describe("readFrames", () => {
  /** chunks を 1 個ずつ流す ReadableStream を作る */
  function streamOf(chunks: Uint8Array[]): ReadableStream<Uint8Array> {
    let i = 0;
    return new ReadableStream({
      pull(controller) {
        if (i < chunks.length) controller.enqueue(chunks[i++]);
        else controller.close();
      },
    });
  }

  it("yields a complete typed frame body from a single chunk", async () => {
    const frame = encodeProtocolFrame(message);
    const bodies: Uint8Array[] = [];
    for await (const body of readFrames(streamOf([frame]))) bodies.push(body);
    expect(bodies).toHaveLength(1);
    const decoded = decodeTypedFrame(bodies[0] as Uint8Array);
    expect(decoded.type === "protocol" && decoded.message.method).toBe("Ping");
  });

  it("reassembles a frame split across chunk boundaries", async () => {
    const frame = encodeProtocolFrame(message);
    const chunks = [
      frame.subarray(0, 2),
      frame.subarray(2, 9),
      frame.subarray(9),
    ];
    const bodies: Uint8Array[] = [];
    for await (const body of readFrames(streamOf(chunks))) bodies.push(body);
    expect(bodies).toHaveLength(1);
    const decoded = decodeTypedFrame(bodies[0] as Uint8Array);
    expect(decoded.type === "protocol" && decoded.message.method).toBe("Ping");
  });

  it("yields multiple frames concatenated in one chunk", async () => {
    const a = encodeProtocolFrame({ ...message, id: 1, method: "A" });
    const b = encodeProtocolFrame({ ...message, id: 2, method: "B" });
    const merged = new Uint8Array(a.length + b.length);
    merged.set(a, 0);
    merged.set(b, a.length);
    const bodies: Uint8Array[] = [];
    for await (const body of readFrames(streamOf([merged]))) bodies.push(body);
    expect(bodies).toHaveLength(2);
  });
});
