/**
 * Wire byte-compat test (= Phase 6b)。
 *
 * Rust `crates/unison-protocol/tests/test_wire_byte_compat.rs` が emit した
 * reference fixture (= 既知の `ProtocolMessage` を Rust wire encoder で typed
 * frame まで serialize した hex) と、 TS encoder の出力が **byte 一致** する
 * ことを assert する。
 *
 * これにより live connection なしで「TS が Rust と同じ wire を喋る」ことを
 * 証明する (= real browser↔server round-trip は Phase 6d)。
 *
 * 注: fixture の packet header は `timestamp = 0` 固定 (= Rust 側で決定的に
 * 組んでいる)、 TS の `encodePacket` も timestamp を常に 0 にするため一致する。
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { encodeProtocolFrame, decodeTypedFrame } from "../../src/channel/frame.js";
import {
  MSG_TYPE_EVENT,
  MSG_TYPE_REQUEST,
  type ProtocolMessage,
} from "../../src/wire/protocol_message.js";

/** Rust fixture (= hex 文字列) の所在 */
const FIXTURE_DIR = fileURLToPath(
  new URL(
    "../../../../crates/unison-protocol/tests/fixtures/wire/",
    import.meta.url,
  ),
);

/** fixture hex を Uint8Array へ */
function loadFixture(name: string): Uint8Array {
  const hex = readFileSync(`${FIXTURE_DIR}${name}`, "utf-8").trim();
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

/** Uint8Array を lowercase hex へ (= 差分を読みやすく) */
function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

const textEncoder = new TextEncoder();

describe("wire byte-compat: TS frame == Rust fixture", () => {
  it("produces byte-identical bytes for a channel request frame", () => {
    const msg: ProtocolMessage = {
      id: 7,
      method: "SubscribeMetric",
      msgType: MSG_TYPE_REQUEST,
      payload: textEncoder.encode('{"names":["cpu","memory"]}'),
    };
    const tsFrame = encodeProtocolFrame(msg);
    const rustFrame = loadFixture("request_frame.hex");
    expect(toHex(tsFrame)).toBe(toHex(rustFrame));
  });

  it("produces byte-identical bytes for an event frame", () => {
    const msg: ProtocolMessage = {
      id: 0,
      method: "MetricUpdate",
      msgType: MSG_TYPE_EVENT,
      payload: textEncoder.encode('{"name":"cpu","value":42}'),
    };
    const tsFrame = encodeProtocolFrame(msg);
    const rustFrame = loadFixture("event_frame.hex");
    expect(toHex(tsFrame)).toBe(toHex(rustFrame));
  });

  it("produces byte-identical bytes for an __identity frame", () => {
    const identityJson =
      '{"name":"test-server","version":"1.0.0","namespace":"club.chronista.test","channels":[],"metadata":null}';
    const msg: ProtocolMessage = {
      id: 0,
      method: "__identity",
      msgType: MSG_TYPE_EVENT,
      payload: textEncoder.encode(identityJson),
    };
    const tsFrame = encodeProtocolFrame(msg);
    const rustFrame = loadFixture("identity_frame.hex");
    expect(toHex(tsFrame)).toBe(toHex(rustFrame));
  });

  it("decodes the Rust request fixture back into the original message", () => {
    const rustFrame = loadFixture("request_frame.hex");
    // 4B length prefix を剥がして typed frame body へ
    const body = rustFrame.subarray(4);
    const decoded = decodeTypedFrame(body);
    expect(decoded.type).toBe("protocol");
    if (decoded.type !== "protocol") throw new Error("unreachable");
    expect(decoded.message.id).toBe(7);
    expect(decoded.message.method).toBe("SubscribeMetric");
    expect(decoded.message.msgType).toBe(MSG_TYPE_REQUEST);
    expect(new TextDecoder().decode(decoded.message.payload)).toBe(
      '{"names":["cpu","memory"]}',
    );
  });
});
