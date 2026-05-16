/**
 * ProtoCodec (= Phase 2d) — protobuf wire codec。
 *
 * `@bufbuild/protobuf` の `toBinary` / `fromBinary` を wrap し、 Rust server 側
 * の buffa (= proto3 互換シリアライゼーション) と byte 互換な wire を生成する。
 *
 * buf protobuf v2 は descriptor 駆動 (= `DescMessage` schema が encode/decode の
 * 起点) のため、 codec instance は 1 個の message schema に束縛される。 schema は
 * KDL → TS codegen が出力する proto descriptor で、 channel ごとに供給される
 * (= codegen 拡張、 design §5.2)。
 */

import type { DescMessage, MessageShape } from "@bufbuild/protobuf";
import { fromBinary, toBinary } from "@bufbuild/protobuf";
import { type Codec, CodecError } from "./codec.js";

/**
 * 1 個の proto message schema に束縛された `Codec`。
 *
 * caller は生成コードの `DescMessage` を渡して instance を得る:
 *
 * ```typescript
 * import { MetricUpdateSchema } from "./generated/vp-protocol";
 * const codec = new ProtoCodec(MetricUpdateSchema);
 * ```
 */
export class ProtoCodec<Desc extends DescMessage>
  implements Codec<MessageShape<Desc>>
{
  readonly format = "proto" as const;

  /** @param schema codegen 出力の proto message descriptor */
  constructor(private readonly schema: Desc) {}

  encode(value: MessageShape<Desc>): Uint8Array {
    try {
      return toBinary(this.schema, value);
    } catch (cause) {
      throw new CodecError(
        `proto encode failed for ${this.schema.typeName}: ${describe(cause)}`,
        { cause },
      );
    }
  }

  decode(bytes: Uint8Array): MessageShape<Desc> {
    try {
      return fromBinary(this.schema, bytes);
    } catch (cause) {
      throw new CodecError(
        `proto decode failed for ${this.schema.typeName}: ${describe(cause)}`,
        { cause },
      );
    }
  }
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
