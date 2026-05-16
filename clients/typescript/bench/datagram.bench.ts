/**
 * Datagram primitive benchmark (= v1.0.0-alpha.2 baseline)。
 *
 * - LEB128 varint の encode / decode (`src/channel/varint.ts`)。 1 byte / 多 byte
 *   双方を計測する (= channelId prefix の hot path)。
 * - `DispatcherInner` の demux + fan-out (`src/channel/dispatcher.ts`)。 N datagram
 *   を M channel に振り分けるコストを 1 op = N dispatch で計測する。
 *
 * dispatcher bench の sink は no-op push (= AsyncQueue 経由しない) にして、
 * varint decode + Map lookup + subarray の純粋な demux コストを測る。
 */

import { bench, describe } from "vitest";
import { type DatagramSink, DispatcherInner } from "../src/channel/dispatcher.js";
import { decodeVarint, encodeVarint } from "../src/channel/varint.js";

// --- varint encode / decode --------------------------------------------------
const oneByteValue = 100; // < 128 → 1 byte
const multiByteValue = 300_000; // → 3 byte
const oneByteBytes = encodeVarint(oneByteValue);
const multiByteBytes = encodeVarint(multiByteValue);

describe("varint encode", () => {
  bench("1-byte value (< 128)", () => void encodeVarint(oneByteValue));
  bench("3-byte value", () => void encodeVarint(multiByteValue));
});

describe("varint decode", () => {
  bench("1-byte value", () => void decodeVarint(oneByteBytes));
  bench("3-byte value", () => void decodeVarint(multiByteBytes));
});

// --- dispatcher demux + fan-out ----------------------------------------------
// M channel を登録し、 N datagram を作って 1 op で全 dispatch する。
const CHANNELS = 8;
const DATAGRAMS = 256;

function buildDispatcher(): { inner: DispatcherInner; received: { count: number } } {
  const inner = new DispatcherInner();
  const received = { count: 0 };
  for (let id = 0; id < CHANNELS; id++) {
    const sink: DatagramSink = {
      push: () => {
        received.count++;
      },
      end: () => {},
    };
    inner.register(id, sink);
  }
  return { inner, received };
}

// 事前生成: [varint channelId][8B payload] の datagram 群 (= channel をラウンドロビン)
const datagrams: Uint8Array[] = [];
for (let i = 0; i < DATAGRAMS; i++) {
  const channelId = i % CHANNELS;
  const prefix = encodeVarint(channelId);
  const dg = new Uint8Array(prefix.length + 8);
  dg.set(prefix, 0);
  dg.set([1, 2, 3, 4, 5, 6, 7, 8], prefix.length);
  datagrams.push(dg);
}

const { inner } = buildDispatcher();

describe("dispatcher demux + fan-out", () => {
  bench(`${DATAGRAMS} datagrams across ${CHANNELS} channels`, () => {
    for (const dg of datagrams) inner.dispatch(dg);
  });
});
