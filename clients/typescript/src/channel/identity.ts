/**
 * Identity handshake — server 自己紹介の受信 (= Phase 6b)。
 *
 * Unison server は接続直後に bidi stream を 1 本 open し、 そこへ
 * `__identity` の `ProtocolMessage` (= msgType event、 payload は JSON 化した
 * `ServerIdentity`) を 1 本送って stream を finish する
 * (= Rust `quic.rs::handle_connection` の identity 送出ロジック)。
 *
 * TS client は `connect()` 時にこの stream を accept し identity を読む。
 */

import { decodeTypedFrame, readFrames } from "./frame.js";
import type { BidiStream, Connection } from "../transport/types.js";

/** identity route 名 (= Rust `__identity`) */
const IDENTITY_METHOD = "__identity";

/** identity handshake のデフォルト timeout */
export const DEFAULT_IDENTITY_TIMEOUT_MS = 5_000;

/** channel の方向 (= Rust `ChannelDirection`、 snake_case) */
export type ChannelDirection =
  | "server_to_client"
  | "client_to_server"
  | "bidirectional";

/** channel の状態 (= Rust `ChannelStatus`、 snake_case) */
export type ChannelStatus = "available" | "busy" | "unavailable";

/** server が advertise する 1 channel の情報 (= Rust `ChannelInfo`) */
export interface ChannelInfo {
  name: string;
  direction: ChannelDirection;
  lifetime: string;
  status: ChannelStatus;
}

/** server の自己紹介情報 (= Rust `ServerIdentity`) */
export interface ServerIdentity {
  name: string;
  version: string;
  namespace: string;
  channels: ChannelInfo[];
  metadata: unknown;
}

const textDecoder = new TextDecoder("utf-8", { fatal: true });

/**
 * 1 本の identity stream を drain して `ServerIdentity` を返す。
 *
 * stream の最初の PROTOCOL frame を読み、 `__identity` method なら payload を
 * JSON parse する。 frame が無い / method 不一致なら reject。
 */
export async function readIdentity(stream: BidiStream): Promise<ServerIdentity> {
  for await (const body of readFrames(stream.readable)) {
    const decoded = decodeTypedFrame(body);
    if (decoded.type !== "protocol") {
      throw new Error(
        `identity: expected a protocol frame, got a ${decoded.type} frame`,
      );
    }
    const msg = decoded.message;
    if (msg.method !== IDENTITY_METHOD) {
      throw new Error(
        `identity: expected method "${IDENTITY_METHOD}", got "${msg.method}"`,
      );
    }
    try {
      return JSON.parse(textDecoder.decode(msg.payload)) as ServerIdentity;
    } catch (cause) {
      throw new Error("identity: payload is not valid ServerIdentity JSON", {
        cause,
      });
    }
  }
  throw new Error("identity: stream closed before any frame arrived");
}

/**
 * connection が server から identity stream を受けるのを待ち、 `ServerIdentity`
 * を返す。 `timeoutMs` 内に来なければ reject する。
 *
 * `connect()` の中で 1 回だけ呼ぶ (= identity stream は接続あたり 1 本)。
 */
export async function performIdentityHandshake(
  connection: Connection,
  timeoutMs: number = DEFAULT_IDENTITY_TIMEOUT_MS,
): Promise<ServerIdentity> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      reject(
        new Error(`identity handshake timed out after ${timeoutMs}ms`),
      );
    }, timeoutMs);
  });
  const handshake = (async (): Promise<ServerIdentity> => {
    const stream = await connection.acceptBidiStream();
    if (stream === undefined) {
      throw new Error(
        "identity handshake failed: connection closed before the " +
          "server opened its identity stream",
      );
    }
    return readIdentity(stream);
  })();
  try {
    return await Promise.race([handshake, timeout]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
