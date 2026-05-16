import { create } from "@bufbuild/protobuf";
import { StringValueSchema } from "@bufbuild/protobuf/wkt";
import { describe, expect, it } from "vitest";
import { CodecError } from "../../src/codec/codec.js";
import { ProtoCodec } from "../../src/codec/proto_codec.js";

// proto descriptor codegen はまだ無いため (= design §5.2 deferred)、 well-known
// type `StringValue` を schema fixture として使い codec の round-trip を検証する。
describe("ProtoCodec", () => {
  const codec = new ProtoCodec(StringValueSchema);

  it("reports the proto wire format", () => {
    expect(codec.format).toBe("proto");
  });

  it("round-trips a proto message", () => {
    const original = create(StringValueSchema, { value: "hello" });
    const decoded = codec.decode(codec.encode(original));
    expect(decoded.value).toBe("hello");
  });

  it("produces proto3 wire bytes (field 1, len-delimited)", () => {
    const bytes = codec.encode(create(StringValueSchema, { value: "ab" }));
    // tag = (1 << 3) | 2 = 0x0a, len = 2, "ab"
    expect(Array.from(bytes)).toEqual([0x0a, 0x02, 0x61, 0x62]);
  });

  it("throws CodecError on malformed proto bytes", () => {
    // 0x0a (len-delimited tag) followed by a length that overruns the buffer
    expect(() => codec.decode(new Uint8Array([0x0a, 0x7f]))).toThrow(CodecError);
  });
});
