import { describe, expect, it } from "vitest";
import { decodeVarint, encodeVarint, VARINT_MAX_LEN } from "../../src/channel/varint.js";

describe("varint", () => {
  it("encodes small values to one byte", () => {
    expect([...encodeVarint(0)]).toEqual([0x00]);
    expect([...encodeVarint(1)]).toEqual([0x01]);
    expect([...encodeVarint(127)]).toEqual([0x7f]);
  });

  it("encodes medium values to two bytes (matches Rust)", () => {
    expect([...encodeVarint(128)]).toEqual([0x80, 0x01]);
    expect([...encodeVarint(300)]).toEqual([0xac, 0x02]);
  });

  it("round-trips a range of values", () => {
    for (const v of [0, 1, 42, 127, 128, 300, 16383, 16384, 1_000_000]) {
      const enc = encodeVarint(v);
      const dec = decodeVarint(enc);
      expect(dec).toEqual({ value: v, consumed: enc.length });
    }
  });

  it("stops at the terminator, leaving trailing bytes", () => {
    const dec = decodeVarint(Uint8Array.from([0x01, 0xff, 0xff, 0xff]));
    expect(dec).toEqual({ value: 1, consumed: 1 });
  });

  it("rejects malformed (too long) encodings", () => {
    expect(decodeVarint(new Uint8Array(VARINT_MAX_LEN + 1).fill(0xff))).toBeNull();
  });

  it("rejects premature EOF", () => {
    expect(decodeVarint(Uint8Array.from([0x80, 0x80, 0x80]))).toBeNull();
  });

  it("rejects negative input", () => {
    expect(() => encodeVarint(-1)).toThrow(RangeError);
  });
});
