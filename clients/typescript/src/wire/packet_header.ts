/**
 * `PacketHeader` proto3 codec (= Phase 6b)。
 *
 * Rust `crates/unison-protocol/proto/protocol.proto` の `PacketHeader` message
 * (= 固定 protocol message) と byte 一致する hand-written codec。 buffa が出力
 * する proto3 wire (= implicit presence、 zero field skip) を厳守する。
 *
 * field map (proto.PacketHeader):
 *   1  uint32  version
 *   2  uint32  packet_type
 *   3  uint32  flags
 *   4  uint32  payload_length
 *   5  uint32  compressed_length
 *   6  uint64  sequence_number
 *   7  uint64  timestamp
 *   8  uint64  stream_id
 *   9  uint64  message_id
 *   10 uint64  response_to
 *   11 bytes   correlation_id  (= UUID v7 の 16 byte raw、 空なら未設定)
 */

import { ProtoReader, ProtoWriter } from "./proto.js";

/** プロトコルバージョン (= Rust `UnisonPacketHeader::CURRENT_VERSION`) */
export const PACKET_VERSION = 0x01;

/** PacketType enum (= Rust `PacketType` を u8 cast した値) */
export const PACKET_TYPE_DATA = 0x00;
export const PACKET_TYPE_CONTROL = 0x01;
export const PACKET_TYPE_HEARTBEAT = 0x02;
export const PACKET_TYPE_HANDSHAKE = 0x03;

/** PacketFlags bit (= Rust `PacketFlags`) */
export const FLAG_COMPRESSED = 0x0001;

/**
 * `proto.PacketHeader` の TS 表現。
 *
 * 数値 field は number で扱う (= timestamp の ns は 2^53 未満で安全に収まる)。
 * `correlationId` は 16 byte の UUID raw、 未設定なら長さ 0。
 */
export interface PacketHeader {
  version: number;
  packetType: number;
  flags: number;
  payloadLength: number;
  compressedLength: number;
  sequenceNumber: number;
  timestamp: number;
  streamId: number;
  messageId: number;
  responseTo: number;
  correlationId: Uint8Array;
}

/** default 値で `PacketHeader` を作る (= version + packet_type 以外は zero) */
export function newPacketHeader(
  packetType: number = PACKET_TYPE_DATA,
): PacketHeader {
  return {
    version: PACKET_VERSION,
    packetType,
    flags: 0,
    payloadLength: 0,
    compressedLength: 0,
    sequenceNumber: 0,
    timestamp: 0,
    streamId: 0,
    messageId: 0,
    responseTo: 0,
    correlationId: new Uint8Array(0),
  };
}

/** `PacketHeader` を proto3 byte 列へ encode (= field 番号昇順) */
export function encodePacketHeader(h: PacketHeader): Uint8Array {
  const w = new ProtoWriter();
  w.uint32(1, h.version);
  w.uint32(2, h.packetType);
  w.uint32(3, h.flags);
  w.uint32(4, h.payloadLength);
  w.uint32(5, h.compressedLength);
  w.uint64(6, h.sequenceNumber);
  w.uint64(7, h.timestamp);
  w.uint64(8, h.streamId);
  w.uint64(9, h.messageId);
  w.uint64(10, h.responseTo);
  w.bytes(11, h.correlationId);
  return w.finish();
}

/** proto3 byte 列から `PacketHeader` を decode (= 未設定 field は default) */
export function decodePacketHeader(bytes: Uint8Array): PacketHeader {
  const h = newPacketHeader();
  // version も含めて全 field を読み込み直す (= wire 値が source of truth)
  h.version = 0;
  h.packetType = 0;
  const r = new ProtoReader(bytes);
  for (let f = r.next(); f !== null; f = r.next()) {
    switch (f.fieldNo) {
      case 1:
        h.version = Number(f.varint ?? 0n);
        break;
      case 2:
        h.packetType = Number(f.varint ?? 0n);
        break;
      case 3:
        h.flags = Number(f.varint ?? 0n);
        break;
      case 4:
        h.payloadLength = Number(f.varint ?? 0n);
        break;
      case 5:
        h.compressedLength = Number(f.varint ?? 0n);
        break;
      case 6:
        h.sequenceNumber = Number(f.varint ?? 0n);
        break;
      case 7:
        h.timestamp = Number(f.varint ?? 0n);
        break;
      case 8:
        h.streamId = Number(f.varint ?? 0n);
        break;
      case 9:
        h.messageId = Number(f.varint ?? 0n);
        break;
      case 10:
        h.responseTo = Number(f.varint ?? 0n);
        break;
      case 11:
        h.correlationId = f.bytes ?? new Uint8Array(0);
        break;
      default:
        break; // 未知 field は無視 (= forward compat)
    }
  }
  return h;
}

/** compressed フラグが立っているか */
export function isCompressed(h: PacketHeader): boolean {
  return h.compressedLength > 0 && (h.flags & FLAG_COMPRESSED) !== 0;
}

/** payload bytes の実 size (= 圧縮時は compressed_length) */
export function actualPayloadSize(h: PacketHeader): number {
  return h.compressedLength > 0 ? h.compressedLength : h.payloadLength;
}
