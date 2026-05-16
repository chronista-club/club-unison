/**
 * TrustMode → WebTransportOptions の変換 (= Phase 2b)。
 *
 * browser WebTransport の trust path は 2 つだけ — 標準 CA 検証 ("system") と
 * cert hash pinning ({certHash})。 後者は `serverCertificateHashes` に対応する。
 */

import type { TrustMode } from "./types.js";

/** SHA-256 = 32 bytes = 64 hex chars */
const CERT_HASH_HEX_LEN = 64;

/**
 * hex 文字列を Uint8Array に変換 (= 不正な長さ / 文字は throw)。
 *
 * 戻り型は `Uint8Array<ArrayBuffer>` を明示 — `WebTransportHash.value` の
 * `BufferSource` は ArrayBuffer-backed view のみ受ける (= TS 5.7 generic 厳格化)。
 */
function hexToBytes(hex: string): Uint8Array<ArrayBuffer> {
  if (hex.length !== CERT_HASH_HEX_LEN) {
    throw new Error(
      `certHash must be ${CERT_HASH_HEX_LEN} hex chars (SHA-256); got ${hex.length}`,
    );
  }
  if (!/^[0-9a-fA-F]+$/.test(hex)) {
    throw new Error("certHash contains non-hex characters");
  }
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/** TrustMode を WebTransport コンストラクタの options に変換する */
export function buildWebTransportOptions(
  trust: TrustMode | undefined,
): WebTransportOptions {
  if (trust === undefined || trust === "system") {
    return {};
  }
  return {
    serverCertificateHashes: [
      { algorithm: "sha-256", value: hexToBytes(trust.certHash) },
    ],
  };
}
