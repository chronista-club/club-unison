/**
 * Channel payload の default codec (= Phase 2c)。
 *
 * design §5.1 の通り全 channel が default で JSON wire。 `JsonCodec.shared` は
 * 構造的で状態を持たないため、 `ChannelPayload` 型に narrow した 1 instance を
 * 全 channel で再利用する (= ProtoCodec は KDL→proto-descriptor codegen 待ち)。
 */

import type { Codec } from "../codec/codec.js";
import { JsonCodec } from "../codec/json_codec.js";
import type { ChannelPayload } from "./types.js";

/** 全 channel 共有の default payload codec (= JsonCodec.shared を型 narrow) */
export const defaultCodec: Codec<ChannelPayload> =
  JsonCodec.shared as Codec<ChannelPayload>;
