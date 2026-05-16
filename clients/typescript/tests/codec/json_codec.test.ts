import { describe, expect, it } from "vitest";
import { CodecError } from "../../src/codec/codec.js";
import { JsonCodec } from "../../src/codec/json_codec.js";

describe("JsonCodec", () => {
  it("reports the json wire format", () => {
    expect(JsonCodec.shared.format).toBe("json");
  });

  it("round-trips a structured value", () => {
    const codec = new JsonCodec<{ name: string; value: number }>();
    const original = { name: "cpu", value: 42 };
    const decoded = codec.decode(codec.encode(original));
    expect(decoded).toEqual(original);
  });

  it("encodes to a Uint8Array of UTF-8 JSON", () => {
    const bytes = JsonCodec.shared.encode({ a: 1 });
    expect(bytes).toBeInstanceOf(Uint8Array);
    expect(new TextDecoder().decode(bytes)).toBe('{"a":1}');
  });

  it("throws CodecError on non-serializable input (circular ref)", () => {
    const circular: Record<string, unknown> = {};
    circular["self"] = circular;
    expect(() => JsonCodec.shared.encode(circular)).toThrow(CodecError);
  });

  it("throws CodecError on malformed JSON bytes", () => {
    expect(() => JsonCodec.shared.decode(new TextEncoder().encode("{bad"))).toThrow(
      CodecError,
    );
  });

  it("throws CodecError on invalid UTF-8 bytes", () => {
    expect(() => JsonCodec.shared.decode(new Uint8Array([0xff, 0xfe]))).toThrow(
      CodecError,
    );
  });
});
