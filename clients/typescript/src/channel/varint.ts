/**
 * LEB128 varint (= Phase 2c)。
 *
 * datagram channel の `channel_id` prefix encoding。 Rust 側
 * `network/datagram_channel.rs` の `encode_varint` / `decode_varint` と wire 互換。
 * proto3 と同じ unsigned LEB128。
 */

/** u64 を表す varint の最大 byte 数 (= LEB128 で 64 bit) */
export const VARINT_MAX_LEN = 10;

/** `value` を LEB128 varint として encode (= 非負整数のみ) */
export function encodeVarint(value: number): Uint8Array {
  if (!Number.isInteger(value) || value < 0) {
    throw new RangeError(`varint requires a non-negative integer, got ${value}`);
  }
  const out: number[] = [];
  let v = value;
  while (v >= 0x80) {
    out.push((v & 0x7f) | 0x80);
    v = Math.floor(v / 0x80);
  }
  out.push(v);
  return Uint8Array.from(out);
}

/**
 * `bytes` 先頭から varint を読み `{ value, consumed }` を返す。
 * malformed (= 10 byte 超え / premature EOF) は `null` (= caller が drop)。
 */
export function decodeVarint(
  bytes: Uint8Array,
): { value: number; consumed: number } | null {
  let value = 0;
  let shift = 1;
  for (let i = 0; i < VARINT_MAX_LEN && i < bytes.length; i++) {
    const b = bytes[i] as number;
    value += (b & 0x7f) * shift;
    if ((b & 0x80) === 0) return { value, consumed: i + 1 };
    shift *= 0x80;
  }
  return null;
}
