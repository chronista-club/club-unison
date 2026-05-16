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
export { WebTransportClient, WebTransportConnection, connect } from "./transport/web_transport.js";

// === Phase 2c: channel ===
export type {
  ChannelMeta,
  ChannelPayload,
  DatagramChannel,
  DatagramChannelMeta,
  EventName,
  RequestName,
  UnisonChannel,
} from "./channel/types.js";
export { UnisonChannelImpl } from "./channel/unison_channel.js";
export { DatagramChannelImpl } from "./channel/datagram_channel.js";
export { DatagramDispatcher, DispatcherInner } from "./channel/dispatcher.js";

// === Phase 2d: codec ===
export type { Codec, CodecFormat } from "./codec/codec.js";
export { CodecError } from "./codec/codec.js";
export { JsonCodec } from "./codec/json_codec.js";
export { ProtoCodec } from "./codec/proto_codec.js";

// === Top-level API === (= 未実装、 placeholder)
// export const unisonClient = {
//   async connect(opts: ConnectOptions): Promise<UnisonClient> {
//     // Phase 2b で実装
//     throw new Error("not yet implemented (v1.0.0-alpha.1)");
//   },
// };

// SDK version (= package.json と同期)
export const VERSION = "1.0.0-alpha.2";
