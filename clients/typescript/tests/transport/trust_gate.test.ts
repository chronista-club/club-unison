import { describe, expect, it } from "vitest";
import { enforceTrustGate } from "../../src/transport/web_transport.js";

const HASH_TRUST = {
  certHash:
    "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
} as const;

describe("enforceTrustGate", () => {
  it("allows cert pinning on IPv4 loopback", () => {
    expect(() =>
      enforceTrustGate("https://127.0.0.1:4439", HASH_TRUST),
    ).not.toThrow();
  });

  it("allows cert pinning on localhost", () => {
    expect(() =>
      enforceTrustGate("https://localhost:4439", HASH_TRUST),
    ).not.toThrow();
  });

  // 回帰: URL.hostname は IPv6 を `[::1]` と角括弧付きで返すため、
  // 正規化前は `::1` と照合 miss し loopback でも throw していた。
  it("allows cert pinning on bracketed IPv6 loopback [::1]", () => {
    expect(() =>
      enforceTrustGate("https://[::1]:4439", HASH_TRUST),
    ).not.toThrow();
  });

  it("rejects cert pinning on a non-loopback host", () => {
    expect(() => enforceTrustGate("https://example.com:443", HASH_TRUST)).toThrow(
      /restricted to localhost/,
    );
  });

  it("rejects cert pinning on a non-loopback IPv6 host", () => {
    expect(() =>
      enforceTrustGate("https://[2001:db8::1]:443", HASH_TRUST),
    ).toThrow(/restricted to localhost/);
  });

  it("skips the gate for system trust and undefined", () => {
    expect(() =>
      enforceTrustGate("https://example.com:443", "system"),
    ).not.toThrow();
    expect(() =>
      enforceTrustGate("https://example.com:443", undefined),
    ).not.toThrow();
  });
});
