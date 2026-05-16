/**
 * @chronista-club/unison-client — TS SDK public entry
 *
 * v1.0 polyglot client base (= Phase 2a 雛形)。
 *
 * Phase 2b 以降で `transport/` / `channel/` / `codec/` の各 module を実装し、
 * ここから re-export する pattern。 現在は **empty entry**、 caller が import しても
 * 何も提供しない (= alpha.1 status の正直な signal)。
 *
 * See `design/typescript-client-api.md` for the API contract this package will satisfy.
 */

// === Phase 5: error category framework (UNS-15) ===
export type { ErrorCategory } from "./error/category.js";
export { ERROR_CATEGORIES } from "./error/category.js";

// === Phase 2b: transport === (= 実装中)
export type {
  BidiStream,
  Connection,
  ConnectionEvent,
  ConnectOptions,
  Transport,
  TrustMode,
} from "./transport/types.js";
export {
  UnisonTransportError,
  WebTransportUnsupportedError,
} from "./transport/errors.js";
export {
  WebTransportClient,
  WebTransportConnection,
  // transport-level の低レベル connect (= URL 直結、 facade を介さない)。
  // caller-facing entry は client.ts の `connect` (下記参照)。
  connect as connectTransport,
} from "./transport/web_transport.js";

// === Phase 2c: channel ===
export type {
  ChannelMeta,
  ChannelPayload,
  ChannelTypeMap,
  DatagramChannel,
  DatagramChannelMeta,
  EventName,
  EventPayload,
  EventType,
  RequestName,
  RequestType,
  ResponseType,
  UnisonChannel,
} from "./channel/types.js";
export { UnisonChannelImpl } from "./channel/unison_channel.js";
export { DatagramChannelImpl } from "./channel/datagram_channel.js";
export { DatagramDispatcher, DispatcherInner } from "./channel/dispatcher.js";

// === Phase 6b: Rust-compatible wire format ===
export {
  FRAME_TYPE_PROTOCOL,
  FRAME_TYPE_RAW,
  decodeTypedFrame,
  encodeProtocolFrame,
  encodeRawFrame,
} from "./channel/frame.js";
export type { DecodedFrame } from "./channel/frame.js";
export { decodePacket, encodePacket } from "./wire/packet.js";
export type { DecodedPacket, PacketOptions } from "./wire/packet.js";
export {
  PACKET_VERSION,
  decodePacketHeader,
  encodePacketHeader,
  newPacketHeader,
} from "./wire/packet_header.js";
export type { PacketHeader } from "./wire/packet_header.js";
export {
  MSG_TYPE_ERROR,
  MSG_TYPE_EVENT,
  MSG_TYPE_REQUEST,
  MSG_TYPE_RESPONSE,
  decodeProtocolMessage,
  encodeProtocolMessage,
  messageTypeName,
  messageTypeValue,
} from "./wire/protocol_message.js";
export type { MessageTypeName, ProtocolMessage } from "./wire/protocol_message.js";

// === Phase 6b: identity handshake ===
export {
  DEFAULT_IDENTITY_TIMEOUT_MS,
  performIdentityHandshake,
  readIdentity,
} from "./channel/identity.js";
export type {
  ChannelDirection,
  ChannelInfo,
  ChannelStatus,
  ServerIdentity,
} from "./channel/identity.js";

// === Phase 2d: codec ===
export type { Codec, CodecFormat } from "./codec/codec.js";
export { CodecError } from "./codec/codec.js";
export { JsonCodec } from "./codec/json_codec.js";
export { ProtoCodec } from "./codec/proto_codec.js";

// === Top-level API === (= Phase 3b、 UnisonClient facade)
// `connect` が caller-facing の primary entry (= design §3.1/§4.1)。
export type { UnisonConnectOptions } from "./client.js";
export { UnisonClient, connect } from "./client.js";

// SDK version (= package.json と同期)
export const VERSION = "1.0.0-rc.1";
