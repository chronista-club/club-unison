/**
 * wire codec unit test (= Phase 6b、 t-wada pyramid の Small 層)。
 *
 * `PacketHeader` / `ProtocolMessage` の proto3 codec round-trip と、 proto3
 * implicit-presence (= zero / 空 field の skip) を検証する。 zero skip は Rust
 * buffa との byte 一致の要。
 */

import { describe, expect, it } from "vitest";
import {
  decodePacketHeader,
  encodePacketHeader,
  newPacketHeader,
} from "../../src/wire/packet_header.js";
import {
  MSG_TYPE_EVENT,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
  decodeProtocolMessage,
  encodeProtocolMessage,
  messageTypeName,
  messageTypeValue,
  type ProtocolMessage,
} from "../../src/wire/protocol_message.js";
import { decodePacket, encodePacket } from "../../src/wire/packet.js";

describe("PacketHeader proto3 codec", () => {
  it("round-trips a populated header", () => {
    const h = newPacketHeader();
    h.payloadLength = 128;
    h.sequenceNumber = 42;
    h.streamId = 7;
    h.messageId = 99;
    h.responseTo = 3;
    h.correlationId = new Uint8Array(16).fill(0xab);
    const restored = decodePacketHeader(encodePacketHeader(h));
    expect(restored).toEqual(h);
  });

  it("skips zero-valued fields (proto3 implicit presence)", () => {
    // 全 field zero (= version も 0) なら出力は空 byte 列
    const empty = newPacketHeader();
    empty.version = 0;
    empty.packetType = 0;
    expect(encodePacketHeader(empty)).toEqual(new Uint8Array(0));
  });

  it("encodes only version when only version is non-zero", () => {
    const h = newPacketHeader();
    h.version = 1; // packetType=0 → skip
    // field 1 (version) tag = (1<<3)|0 = 0x08, value 1
    expect([...encodePacketHeader(h)]).toEqual([0x08, 0x01]);
  });
});

describe("ProtocolMessage proto3 codec", () => {
  it("round-trips a populated message", () => {
    const m: ProtocolMessage = {
      id: 7,
      method: "SubscribeMetric",
      msgType: MSG_TYPE_RESPONSE,
      payload: new TextEncoder().encode('{"ok":true}'),
    };
    const restored = decodeProtocolMessage(encodeProtocolMessage(m));
    expect(restored).toEqual(m);
  });

  it("skips id=0 and msgType=REQUEST(0) (proto3 implicit presence)", () => {
    const m: ProtocolMessage = {
      id: 0,
      method: "Ping",
      msgType: MSG_TYPE_REQUEST,
      payload: new Uint8Array(0),
    };
    const bytes = encodeProtocolMessage(m);
    // field 2 (method) のみ: tag 0x12, len 4, "Ping"
    expect([...bytes]).toEqual([0x12, 0x04, 0x50, 0x69, 0x6e, 0x67]);
  });

  it("maps MessageType enum names <-> values", () => {
    expect(messageTypeName(MSG_TYPE_EVENT)).toBe("event");
    expect(messageTypeValue("response")).toBe(MSG_TYPE_RESPONSE);
  });
});

describe("UnisonPacket layer", () => {
  it("round-trips a payload through the [u32 header_len][header][payload] format", () => {
    const payload = new TextEncoder().encode("hello unison");
    const packet = encodePacket(payload);
    const decoded = decodePacket(packet);
    expect([...decoded.payload]).toEqual([...payload]);
    expect(decoded.header.payloadLength).toBe(payload.length);
    expect(decoded.header.compressedLength).toBe(0);
  });

  it("rejects a packet whose payload size disagrees with the header", () => {
    const packet = encodePacket(new TextEncoder().encode("abc"));
    // payload を 1 byte 削って size 不整合を作る
    expect(() => decodePacket(packet.subarray(0, packet.length - 1))).toThrow();
  });
});
