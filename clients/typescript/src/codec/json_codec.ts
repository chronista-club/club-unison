/**
 * JsonCodec (= Phase 2d) — default wire codec。
 *
 * `JSON.stringify` / `JSON.parse` を `TextEncoder` / `TextDecoder` で
 * Uint8Array に橋渡しする。 Rust 側 `JsonCodec` (= serde_json) と wire 互換。
 *
 * 構造的 codec のため任意のメッセージ型を 1 instance で扱える (= schema 不要)。
 * dev/debug 用途では human-readable な wire を得られるのが利点。
 */

import { type Codec, CodecError } from "./codec.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

/**
 * JSON ベースの `Codec`。
 *
 * 全メッセージ型を構造的に扱えるため `JsonCodec.shared` を再利用すればよい
 * (= 状態を持たない)。
 */
export class JsonCodec<T = unknown> implements Codec<T> {
  readonly format = "json" as const;

  /** 状態を持たない共有 instance (= 全 channel で再利用可) */
  static readonly shared = new JsonCodec();

  encode(value: T): Uint8Array {
    try {
      return encoder.encode(JSON.stringify(value));
    } catch (cause) {
      throw new CodecError(`JSON encode failed: ${describe(cause)}`, { cause });
    }
  }

  decode(bytes: Uint8Array): T {
    let text: string;
    try {
      text = decoder.decode(bytes);
    } catch (cause) {
      throw new CodecError(`invalid UTF-8 in JSON payload: ${describe(cause)}`, {
        cause,
      });
    }
    try {
      return JSON.parse(text) as T;
    } catch (cause) {
      throw new CodecError(`JSON decode failed: ${describe(cause)}`, { cause });
    }
  }
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
