/**
 * Codec hot-path benchmark (= v1.0.0-alpha.2 baseline)。
 *
 * `JsonCodec` / `ProtoCodec` の encode + decode を small / medium / large の
 * 3 payload size で計測する。
 *
 * `ProtoCodec` は proto descriptor codegen がまだ無いため (= design §5.2 deferred)、
 * well-known type `google.protobuf.Struct` を schema fixture として使う。 Struct は
 * JSON 相当の任意オブジェクトを protobuf wire で表現できるため、 同一 payload を
 * JSON / proto 双方で計測でき公平な比較になる。
 *
 * 注意: ProtoCodec の `encode`/`decode` は `MessageShape<Desc>` を入出力とする。
 * 純粋な wire 変換コストを測るため、 `MessageShape` の値 (Struct) は bench loop の
 * 外で `fromJson` 経由で 1 度だけ構築する。
 */

import { fromJson } from "@bufbuild/protobuf";
import { StructSchema } from "@bufbuild/protobuf/wkt";
import { bench, describe } from "vitest";
import { JsonCodec } from "../src/codec/json_codec.js";
import { ProtoCodec } from "../src/codec/proto_codec.js";

/** ~3 フィールドの小 payload */
const small = { id: 42, name: "cpu", value: 0.873 };

/** ~12 フィールドの中 payload (= 典型的な metric update) */
const medium = {
  id: 1024,
  name: "agent-worker-3",
  status: "running",
  cpu: 73.4,
  memory: 512.0,
  uptime: 86_400,
  region: "ap-northeast-1",
  version: "1.0.0-alpha.2",
  healthy: true,
  tags: ["prod", "metric", "hot"],
  lastSeen: "2026-05-16T12:00:00Z",
  meta: { host: "node-7", pid: 4821 },
};

/** ~100 オブジェクトの配列 (= 大 payload、 batch event 想定) */
const large = {
  batch: Array.from({ length: 100 }, (_, i) => ({
    seq: i,
    name: `metric-${i}`,
    value: Math.sin(i),
    ts: 1_747_000_000 + i,
    ok: i % 2 === 0,
  })),
};

const jsonCodec = new JsonCodec();
const protoCodec = new ProtoCodec(StructSchema);

// proto の MessageShape 値を事前構築 (= wire 変換コストのみを計測対象にする)
const smallStruct = fromJson(StructSchema, small);
const mediumStruct = fromJson(StructSchema, medium);
const largeStruct = fromJson(StructSchema, large);

// decode 計測用に encode 済みバイト列を事前準備
const smallJsonBytes = jsonCodec.encode(small);
const mediumJsonBytes = jsonCodec.encode(medium);
const largeJsonBytes = jsonCodec.encode(large);
const smallProtoBytes = protoCodec.encode(smallStruct);
const mediumProtoBytes = protoCodec.encode(mediumStruct);
const largeProtoBytes = protoCodec.encode(largeStruct);

describe("JsonCodec.encode", () => {
  bench("small", () => void jsonCodec.encode(small));
  bench("medium", () => void jsonCodec.encode(medium));
  bench("large", () => void jsonCodec.encode(large));
});

describe("JsonCodec.decode", () => {
  bench("small", () => void jsonCodec.decode(smallJsonBytes));
  bench("medium", () => void jsonCodec.decode(mediumJsonBytes));
  bench("large", () => void jsonCodec.decode(largeJsonBytes));
});

describe("ProtoCodec.encode", () => {
  bench("small", () => void protoCodec.encode(smallStruct));
  bench("medium", () => void protoCodec.encode(mediumStruct));
  bench("large", () => void protoCodec.encode(largeStruct));
});

describe("ProtoCodec.decode", () => {
  bench("small", () => void protoCodec.decode(smallProtoBytes));
  bench("medium", () => void protoCodec.decode(mediumProtoBytes));
  bench("large", () => void protoCodec.decode(largeProtoBytes));
});
