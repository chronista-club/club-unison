/**
 * `UnisonPacket` wire layer (= Phase 6b)。
 *
 * Rust `crates/unison-protocol/src/packet/` の `[u32 BE header_len]
 * [buffa PacketHeader][payload bytes]` packet format と byte 一致する。
 *
 * ```text
 * [u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes]
 * ```
 *
 * `payload` は `ProtocolMessage` を encode した buffa バイト列。 Rust 側は
 * payload ≥ 2048 byte で zstd 圧縮するが、 TS client は **常に非圧縮**で送る
 * (= `compressed_length = 0`、 wire-valid)。 受信時に圧縮 packet が来たら
 * 明示 error を投げる (= Phase 6d で zstd decode を入れるまでの正直な signal)。
 */

import {
  type PacketHeader,
  PACKET_TYPE_DATA,
  actualPayloadSize,
  decodePacketHeader,
  encodePacketHeader,
  isCompressed,
  newPacketHeader,
} from "./packet_header.js";

/** packet header に乗せる任意設定 (= 全 field 既定、 caller が必要分のみ上書き) */
export interface PacketOptions {
  packetType?: number;
  sequenceNumber?: number;
  streamId?: number;
  messageId?: number;
  responseTo?: number;
  correlationId?: Uint8Array;
}

/**
 * `payload` (= encode 済み `ProtocolMessage` バイト列) を 1 本の UnisonPacket
 * バイト列 (= `[u32 BE header_len][PacketHeader][payload]`) へ encode する。
 *
 * 非圧縮固定: `payload_length` に実 size、 `compressed_length = 0`。
 */
export function encodePacket(
  payload: Uint8Array,
  opts: PacketOptions = {},
): Uint8Array {
  const header: PacketHeader = newPacketHeader(
    opts.packetType ?? PACKET_TYPE_DATA,
  );
  header.payloadLength = payload.length;
  header.compressedLength = 0;
  header.sequenceNumber = opts.sequenceNumber ?? 0;
  header.streamId = opts.streamId ?? 0;
  header.messageId = opts.messageId ?? 0;
  header.responseTo = opts.responseTo ?? 0;
  if (opts.correlationId !== undefined) {
    header.correlationId = opts.correlationId;
  }

  const headerBytes = encodePacketHeader(header);
  const out = new Uint8Array(4 + headerBytes.length + payload.length);
  new DataView(out.buffer).setUint32(0, headerBytes.length, false);
  out.set(headerBytes, 4);
  out.set(payload, 4 + headerBytes.length);
  return out;
}

/** packet decode 結果 (= header + 解凍済み payload バイト列) */
export interface DecodedPacket {
  header: PacketHeader;
  payload: Uint8Array;
}

/**
 * UnisonPacket バイト列を header + payload に分解する。
 *
 * 圧縮 packet (= `compressed_length > 0` かつ COMPRESSED flag) は現状 reject
 * する (= TS 側に zstd decode が無い、 Phase 6d で対応)。
 */
export function decodePacket(bytes: Uint8Array): DecodedPacket {
  if (bytes.length < 4) {
    throw new Error("packet: too short for u32 header_len prefix");
  }
  const headerLen = new DataView(
    bytes.buffer,
    bytes.byteOffset,
    bytes.byteLength,
  ).getUint32(0, false);
  if (bytes.length < 4 + headerLen) {
    throw new Error("packet: declared header_len overruns buffer");
  }
  const header = decodePacketHeader(bytes.subarray(4, 4 + headerLen));
  const payloadBytes = bytes.subarray(4 + headerLen);

  const expected = actualPayloadSize(header);
  if (payloadBytes.length !== expected) {
    throw new Error(
      `packet: payload size ${payloadBytes.length} != header-declared ${expected}`,
    );
  }
  if (isCompressed(header)) {
    throw new Error(
      "packet: zstd-compressed payload not supported by TS client yet " +
        "(= Phase 6d adds zstd decode; current fixtures stay < 2KB / uncompressed)",
    );
  }
  return { header, payload: payloadBytes };
}
