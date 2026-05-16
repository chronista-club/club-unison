/**
 * Channel stream の wire frame (= Phase 6b、 Rust server と byte 一致)。
 *
 * `UnisonChannel` (= QUIC / WebTransport bidi stream) を流れる typed frame の
 * encode/decode。 Rust `crates/unison-protocol/src/network/quic.rs` の
 * `read_typed_frame` / `write_typed_frame` と完全一致する layout:
 *
 * ```text
 * [4B BE total_len] [1B frame_type] [payload]
 * ```
 *
 * - `total_len` = frame_type (1B) + payload の合計
 * - `frame_type` = 0x00 PROTOCOL (= UnisonPacket) / 0x01 RAW (= 生 bytes)
 * - PROTOCOL frame の payload = UnisonPacket バイト列
 *   (= `[u32 BE header_len][buffa PacketHeader][buffa ProtocolMessage]`)
 *
 * 旧 Phase 2c の `[4B len][2B hdrLen][JSON header][payload]` 自家製 frame は
 * 廃止 (= Rust server と通信不能だった)。
 */

import { decodePacket, encodePacket, type PacketOptions } from "../wire/packet.js";
import {
  decodeProtocolMessage,
  encodeProtocolMessage,
  type ProtocolMessage,
} from "../wire/protocol_message.js";

/** frame type tag (= Rust `FRAME_TYPE_*`) */
export const FRAME_TYPE_PROTOCOL = 0x00;
export const FRAME_TYPE_RAW = 0x01;

/** typed frame の最大 size (= Rust `MAX_MESSAGE_SIZE`、 8MB) */
const MAX_FRAME_SIZE = 8 * 1024 * 1024;

/**
 * `ProtocolMessage` を 1 本の PROTOCOL typed frame へ encode する。
 *
 * layout: `[4B total_len][0x00][UnisonPacket]`。 `UnisonPacket` の中身は
 * `[u32 header_len][PacketHeader][ProtocolMessage]`。
 */
export function encodeProtocolFrame(
  message: ProtocolMessage,
  packetOpts: PacketOptions = {},
): Uint8Array {
  const msgBytes = encodeProtocolMessage(message);
  const packet = encodePacket(msgBytes, packetOpts);
  return wrapTypedFrame(FRAME_TYPE_PROTOCOL, packet);
}

/** 生 byte 列を 1 本の RAW typed frame へ encode する (= `[4B len][0x01][data]`) */
export function encodeRawFrame(data: Uint8Array): Uint8Array {
  return wrapTypedFrame(FRAME_TYPE_RAW, data);
}

/** type tag + payload を length-prefixed typed frame へ wrap */
function wrapTypedFrame(frameType: number, payload: Uint8Array): Uint8Array {
  const totalLen = 1 + payload.length;
  const frame = new Uint8Array(4 + totalLen);
  new DataView(frame.buffer).setUint32(0, totalLen, false);
  frame[4] = frameType & 0xff;
  frame.set(payload, 5);
  return frame;
}

/** typed frame の decode 結果 */
export type DecodedFrame =
  | { type: "protocol"; message: ProtocolMessage }
  | { type: "raw"; data: Uint8Array };

/**
 * 1 本の typed frame body (= 4B length prefix を剥がした後の `[1B type][payload]`)
 * を decode する。
 */
export function decodeTypedFrame(body: Uint8Array): DecodedFrame {
  if (body.length < 1) {
    throw new Error("frame: empty typed frame (missing type tag)");
  }
  const frameType = body[0] as number;
  const payload = body.subarray(1);
  if (frameType === FRAME_TYPE_PROTOCOL) {
    const { payload: msgBytes } = decodePacket(payload);
    return { type: "protocol", message: decodeProtocolMessage(msgBytes) };
  }
  if (frameType === FRAME_TYPE_RAW) {
    return { type: "raw", data: payload };
  }
  throw new Error(`frame: unknown frame type tag 0x${frameType.toString(16)}`);
}

/**
 * `ReadableStream<Uint8Array>` から typed frame body (= `[1B type][payload]`) を
 * 1 本ずつ取り出す async generator。 byte 跨ぎ chunk を内部 buffer で結合する。
 */
export async function* readFrames(
  readable: ReadableStream<Uint8Array>,
): AsyncGenerator<Uint8Array> {
  const reader = readable.getReader();
  let buffer: Uint8Array<ArrayBufferLike> = new Uint8Array(0);
  try {
    for (;;) {
      // length prefix (4B) が揃うまで読む
      while (buffer.length < 4) {
        const { value, done } = await reader.read();
        if (done) return;
        if (value !== undefined) buffer = concat(buffer, value);
      }
      const totalLen = new DataView(
        buffer.buffer,
        buffer.byteOffset,
        buffer.byteLength,
      ).getUint32(0, false);
      if (totalLen === 0) throw new Error("frame: empty frame");
      if (totalLen > MAX_FRAME_SIZE) {
        throw new Error(`frame: too large (${totalLen} bytes)`);
      }
      // body 全体が揃うまで読む
      while (buffer.length < 4 + totalLen) {
        const { value, done } = await reader.read();
        if (done) return;
        if (value !== undefined) buffer = concat(buffer, value);
      }
      yield buffer.subarray(4, 4 + totalLen);
      buffer = buffer.slice(4 + totalLen);
    }
  } finally {
    reader.releaseLock();
  }
}

function concat(
  a: Uint8Array<ArrayBufferLike>,
  b: Uint8Array<ArrayBufferLike>,
): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}
