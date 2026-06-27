/**
 * Connection-level auth (= v1.4.0)。
 *
 * Rust `enable_auth` / `connect_with_credential` の TS 対応。auth は専用 transport を
 * 要求せず、 reserved `unison.auth` channel を open して `Authenticate` request を 1 回
 * 送るだけ (= stream channel + request を持てば認証できる)。
 *
 * wire 不変条件・各言語 API の SSOT は `design/connection-auth.md` §5.8。
 */

import type { ChannelMeta } from "./types.js";

/** reserved auth channel 名 (= Rust `network::auth::AUTH_CHANNEL_NAME`) */
export const AUTH_CHANNEL_NAME = "unison.auth";

/** credential 提示 method (= Rust `network::auth::AUTHENTICATE_METHOD`) */
export const AUTHENTICATE_METHOD = "Authenticate";

/** `unison.auth` の reserved channel meta (= codegen 不要、 SDK 内蔵) */
export const AUTH_CHANNEL_META = {
  name: AUTH_CHANNEL_NAME,
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: [],
  requests: {
    [AUTHENTICATE_METHOD]: {
      request: "AuthenticateRequest",
      response: "AuthResult",
    },
  },
} as const satisfies ChannelMeta;

/**
 * `Authenticate` request payload (= wire: `{ credential: number[] }`)。
 *
 * `interface` ではなく `type` (= 暗黙 index signature を持ち `ChannelPayload`
 * = `Record<string, unknown>` に代入可能にするため)。
 */
export type AuthenticateRequest = {
  /**
   * opaque credential。u8 の **数値配列** (各要素 0–255)。
   * library は中身を解釈しない (例 Creo ID JWT / API キー / 独自トークン)。
   */
  credential: number[];
};

/** `AuthResult` response payload */
export type AuthResult = {
  ok: boolean;
};

/**
 * credential bytes を wire 互換の request payload に変換する。
 *
 * ⚠️ `Uint8Array` を直接 JSON 化すると `{"0":104,...}` object になり、 Rust 側
 * `Vec<u8>` (= serde_json の数値配列 `[104,...]`) と **非互換**。必ず `Array.from`
 * で `number[]` に変換すること。本関数がその規約を強制する単一の入口。
 */
export function toAuthenticateRequest(credential: Uint8Array): AuthenticateRequest {
  return { credential: Array.from(credential) };
}
