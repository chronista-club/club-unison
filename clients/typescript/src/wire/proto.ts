/**
 * Minimal proto3 wire encoder/decoder (= Phase 6b).
 *
 * Rust server の wire format に byte 一致させるための proto3 primitive。
 * `PacketHeader` / `ProtocolMessage` は固定 protocol message (= user schema では
 * ない) のため、 ここで手書き codec を提供する。 buffa (Rust server 側) は標準
 * proto3 implicit-presence で encode する: scalar が zero / bytes が空のとき
 * field を **skip** する。 ここでも同じ規則を厳守する (= byte 一致の要)。
 *
 * 対応 wire type:
 * - 0 (VARINT)  — uint32 / uint64 / enum
 * - 2 (LEN)     — bytes / string
 *
 * field 番号は昇順で encode する (= buffa の生成 encode が field 宣言順 = 昇順)。
 */

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true });

/** proto3 wire type tag */
const WIRE_VARINT = 0;
const WIRE_LEN = 2;

/** 可変長 byte buffer (= push only、 最後に 1 本の Uint8Array へ) */
export class ProtoWriter {
  #chunks: number[] = [];

  /** 生 byte を 1 個追加 */
  #byte(b: number): void {
    this.#chunks.push(b & 0xff);
  }

  /** LEB128 unsigned varint を追加 (= bigint で 64bit 安全) */
  #varint(value: bigint): void {
    let v = value;
    while (v >= 0x80n) {
      this.#byte(Number(v & 0x7fn) | 0x80);
      v >>= 7n;
    }
    this.#byte(Number(v));
  }

  /** field tag = (fieldNo << 3) | wireType */
  #tag(fieldNo: number, wireType: number): void {
    this.#varint(BigInt((fieldNo << 3) | wireType));
  }

  /** uint32 field (= zero なら skip、 proto3 implicit presence) */
  uint32(fieldNo: number, value: number): void {
    if (value === 0) return;
    this.#tag(fieldNo, WIRE_VARINT);
    this.#varint(BigInt(value >>> 0));
  }

  /** uint64 field (= zero なら skip)。 `value` は number か bigint */
  uint64(fieldNo: number, value: number | bigint): void {
    const v = typeof value === "bigint" ? value : BigInt(Math.trunc(value));
    if (v === 0n) return;
    this.#tag(fieldNo, WIRE_VARINT);
    this.#varint(v);
  }

  /** enum field (= uint32 と同じ wire、 zero なら skip) */
  enum(fieldNo: number, value: number): void {
    this.uint32(fieldNo, value);
  }

  /** bytes field (= 空なら skip) */
  bytes(fieldNo: number, value: Uint8Array): void {
    if (value.length === 0) return;
    this.#tag(fieldNo, WIRE_LEN);
    this.#varint(BigInt(value.length));
    for (const b of value) this.#byte(b);
  }

  /** string field (= 空なら skip) */
  string(fieldNo: number, value: string): void {
    if (value.length === 0) return;
    this.bytes(fieldNo, textEncoder.encode(value));
  }

  /** 蓄積した byte 列を確定する */
  finish(): Uint8Array {
    return Uint8Array.from(this.#chunks);
  }
}

/** 1 field の decode 結果 */
interface ProtoField {
  fieldNo: number;
  wireType: number;
  /** VARINT のときの値 */
  varint?: bigint;
  /** LEN のときの byte 列 */
  bytes?: Uint8Array;
}

/**
 * proto3 byte 列を field 単位で読み出す reader。
 *
 * 未知 field は wire type に応じて skip する (= forward compat)。
 */
export class ProtoReader {
  #buf: Uint8Array;
  #pos = 0;

  constructor(buf: Uint8Array) {
    this.#buf = buf;
  }

  /** 残り byte があるか */
  get hasMore(): boolean {
    return this.#pos < this.#buf.length;
  }

  /** LEB128 varint を 1 個読む */
  #readVarint(): bigint {
    let result = 0n;
    let shift = 0n;
    for (;;) {
      if (this.#pos >= this.#buf.length) {
        throw new Error("proto: varint overruns buffer");
      }
      const b = this.#buf[this.#pos++] as number;
      result |= BigInt(b & 0x7f) << shift;
      if ((b & 0x80) === 0) return result;
      shift += 7n;
      if (shift > 70n) throw new Error("proto: varint too long");
    }
  }

  /** 次の field を 1 個読む (= 無ければ null) */
  next(): ProtoField | null {
    if (!this.hasMore) return null;
    const tag = this.#readVarint();
    const fieldNo = Number(tag >> 3n);
    const wireType = Number(tag & 0x7n);
    if (wireType === WIRE_VARINT) {
      return { fieldNo, wireType, varint: this.#readVarint() };
    }
    if (wireType === WIRE_LEN) {
      const len = Number(this.#readVarint());
      if (this.#pos + len > this.#buf.length) {
        throw new Error("proto: LEN field overruns buffer");
      }
      const bytes = this.#buf.subarray(this.#pos, this.#pos + len);
      this.#pos += len;
      return { fieldNo, wireType, bytes };
    }
    // 未対応 wire type (1 = I64, 5 = I32) — fixed 幅で skip
    if (wireType === 1) {
      this.#pos += 8;
      return { fieldNo, wireType };
    }
    if (wireType === 5) {
      this.#pos += 4;
      return { fieldNo, wireType };
    }
    throw new Error(`proto: unsupported wire type ${wireType}`);
  }
}

/** LEN field の byte 列を UTF-8 string へ */
export function decodeProtoString(bytes: Uint8Array): string {
  return textDecoder.decode(bytes);
}
