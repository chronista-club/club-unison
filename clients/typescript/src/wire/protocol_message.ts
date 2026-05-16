/**
 * `ProtocolMessage` proto3 codec (= Phase 6b)。
 *
 * Rust `proto.ProtocolMessage` (= 全 channel が運ぶ wire-level message) と byte
 * 一致する hand-written codec。 `payload` は channel codec (= JSON / proto) が
 * encode した application message のバイト列。
 *
 * field map (proto.ProtocolMessage):
 *   1  uint64       id        (= Request は一意、 Event は 0 可)
 *   2  string       method    (= "Query" / "__channel:control" / "__identity" 等)
 *   3  MessageType  msg_type  (= enum)
 *   4  bytes        payload
 */

import { ProtoReader, ProtoWriter, decodeProtoString } from "./proto.js";

/** MessageType enum (= Rust `proto.MessageType` と同じ値) */
export const MSG_TYPE_REQUEST = 0;
export const MSG_TYPE_RESPONSE = 1;
export const MSG_TYPE_EVENT = 2;
export const MSG_TYPE_ERROR = 3;

/** MessageType の string 表現 (= 診断用、 SDK API surface) */
export type MessageTypeName = "request" | "response" | "event" | "error";

/** enum 値 → string 名 */
export function messageTypeName(v: number): MessageTypeName {
  switch (v) {
    case MSG_TYPE_RESPONSE:
      return "response";
    case MSG_TYPE_EVENT:
      return "event";
    case MSG_TYPE_ERROR:
      return "error";
    default:
      return "request";
  }
}

/** string 名 → enum 値 */
export function messageTypeValue(name: MessageTypeName): number {
  switch (name) {
    case "response":
      return MSG_TYPE_RESPONSE;
    case "event":
      return MSG_TYPE_EVENT;
    case "error":
      return MSG_TYPE_ERROR;
    default:
      return MSG_TYPE_REQUEST;
  }
}

/** `proto.ProtocolMessage` の TS 表現 */
export interface ProtocolMessage {
  /** message ID (= Request は一意、 Event は 0) */
  id: number;
  /** method 名 (= request/event 名、 または `__channel:` / `__identity` route) */
  method: string;
  /** メッセージ種別 */
  msgType: number;
  /** codec が encode した payload バイト列 */
  payload: Uint8Array;
}

/** `ProtocolMessage` を proto3 byte 列へ encode (= field 番号昇順) */
export function encodeProtocolMessage(m: ProtocolMessage): Uint8Array {
  const w = new ProtoWriter();
  w.uint64(1, m.id);
  w.string(2, m.method);
  w.enum(3, m.msgType);
  w.bytes(4, m.payload);
  return w.finish();
}

/** proto3 byte 列から `ProtocolMessage` を decode (= 未設定 field は default) */
export function decodeProtocolMessage(bytes: Uint8Array): ProtocolMessage {
  const m: ProtocolMessage = {
    id: 0,
    method: "",
    msgType: MSG_TYPE_REQUEST,
    payload: new Uint8Array(0),
  };
  const r = new ProtoReader(bytes);
  for (let f = r.next(); f !== null; f = r.next()) {
    switch (f.fieldNo) {
      case 1:
        m.id = Number(f.varint ?? 0n);
        break;
      case 2:
        m.method = f.bytes !== undefined ? decodeProtoString(f.bytes) : "";
        break;
      case 3:
        m.msgType = Number(f.varint ?? 0n);
        break;
      case 4:
        m.payload = f.bytes ?? new Uint8Array(0);
        break;
      default:
        break; // 未知 field は無視
    }
  }
  return m;
}
