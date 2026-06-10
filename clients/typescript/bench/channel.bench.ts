/**
 * Channel hot-path benchmark (= v1.0.0-alpha.2 baseline)。
 *
 * - `UnisonChannel.request()` の round-trip (= encode → mock transport → server
 *   stub echo → decode) を 1 回 = 1 op で計測する。
 * - `DatagramChannel` の event 配送 throughput (= dispatcher demux → codec decode
 *   → AsyncQueue 配送) を計測する。
 *
 * 実 WebTransport は使わず `src/testing/` の in-memory pipe
 * を再利用する。 transport は memory pipe なので I/O コストはほぼゼロ、 計測対象は
 * SDK 側の frame encode/decode + codec + recv loop dispatch である点に注意。
 */

import { bench, describe } from "vitest";
import { DatagramChannelImpl } from "../src/channel/datagram_channel.js";
import { DatagramDispatcher } from "../src/channel/dispatcher.js";
import type {
  ChannelMeta,
  ChannelPayload,
  DatagramChannelMeta,
} from "../src/channel/types.js";
import { UnisonChannelImpl } from "../src/channel/unison_channel.js";
import { JsonCodec } from "../src/codec/json_codec.js";
import { MockConnection, StreamServerStub } from "../src/testing/index.js";

const codec = JsonCodec.shared as JsonCodec<ChannelPayload>;

const ControlMeta = {
  name: "control",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: ["Notice"],
  requests: {
    Echo: { request: "EchoReq", response: "EchoResp" },
  },
} as const satisfies ChannelMeta;

const MetricMeta = {
  name: "metric",
  backend: "datagram",
  channelId: 1,
  from: "server",
  lifetime: "persistent",
  events: ["Update"],
  requests: {},
} as const satisfies DatagramChannelMeta;

/** request payload (= ~6 フィールド、 典型サイズ) */
const reqPayload: ChannelPayload = {
  names: ["cpu", "memory", "disk"],
  interval: 1000,
  format: "json",
};

// --- UnisonChannel.request() round-trip --------------------------------------
// channel / server stub は 1 度だけ構築し、 bench は request() のみを回す。
const { client, server } = MockConnection.pair();
const clientStream = await client.openBidiStream();
const accepted = await server.acceptStream();
if (accepted.done) throw new Error("server did not accept stream");
const stub = new StreamServerStub(accepted.value, (_method, payload) => payload);
const channel = new UnisonChannelImpl(ControlMeta, clientStream, codec);

describe("UnisonChannel.request", () => {
  bench("round-trip (echo, mock transport)", async () => {
    await channel.request("Echo", reqPayload);
  });
});

// --- DatagramChannel event 配送 throughput -----------------------------------
// server endpoint から datagram を流し、 channel.events() で 1 件受け取る。
const { client: dgClient, server: dgServer } = MockConnection.pair();
const dispatcher = new DatagramDispatcher(dgClient);
const dgChannel = new DatagramChannelImpl(MetricMeta, dgClient, dispatcher, codec);
const dgEvents = dgChannel.events();

/** [varint channelId=1][json payload] datagram を server から送る */
function sendDatagram(payload: ChannelPayload): Promise<void> {
  const body = new TextEncoder().encode(JSON.stringify(payload));
  const buf = new Uint8Array(1 + body.length);
  buf[0] = 1; // channelId=1 は 1 byte varint
  buf.set(body, 1);
  return dgServer.sendDatagram(buf);
}

describe("DatagramChannel event delivery", () => {
  bench("send → demux → decode → deliver one event", async () => {
    await sendDatagram({ name: "cpu", value: 42 });
    await dgEvents.next();
  });
});
