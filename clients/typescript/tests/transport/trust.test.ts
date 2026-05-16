import { describe, expect, it } from "vitest";
import { buildWebTransportOptions } from "../../src/transport/trust.js";

const VALID_HASH =
  "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

describe("buildWebTransportOptions", () => {
  it('"system" produces no serverCertificateHashes', () => {
    expect(
      buildWebTransportOptions("system").serverCertificateHashes,
    ).toBeUndefined();
  });

  it("undefined (default) produces no serverCertificateHashes", () => {
    expect(
      buildWebTransportOptions(undefined).serverCertificateHashes,
    ).toBeUndefined();
  });

  it("{certHash} produces a sha-256 hash with decoded bytes", () => {
    const opts = buildWebTransportOptions({ certHash: VALID_HASH });
    const hashes = opts.serverCertificateHashes;
    expect(hashes).toHaveLength(1);
    expect(hashes![0]!.algorithm).toBe("sha-256");

    const value = hashes![0]!.value as Uint8Array;
    expect(value).toBeInstanceOf(Uint8Array);
    expect(value.length).toBe(32);
    expect(value[0]).toBe(0x00);
    expect(value[1]).toBe(0x11);
    expect(value[31]).toBe(0xff);
  });

  it("rejects a certHash that is not 64 hex chars", () => {
    expect(() => buildWebTransportOptions({ certHash: "abc" })).toThrow();
  });

  it("rejects a certHash with non-hex characters", () => {
    const bad = `zz${VALID_HASH.slice(2)}`;
    expect(() => buildWebTransportOptions({ certHash: bad })).toThrow();
  });
});
