/**
 * DatagramChannel E2E integration test (= Phase 2e、 t-wada pyramid の Medium 層)。
 *
 * mock connection の datagram path → `DatagramDispatcher` の varint demux →
 * 複数 channel への fan-out を実 transport なしで検証する。
 */

import { describe, expect, it } from "vitest";
import { DatagramChannelImpl } from "../../src/channel/datagram_channel.js";
import { DatagramDispatcher } from "../../src/channel/dispatcher.js";
import type { DatagramChannelMeta } from "../../src/channel/types.js";
import type { ChannelPayload } from "../../src/channel/types.js";
import { JsonCodec } from "../../src/codec/json_codec.js";
import { MockConnection } from "./mock_transport.js";

function datagramMeta<const Id extends number, const Name extends string>(
  name: Name,
  channelId: Id,
) {
  return {
    name,
    backend: "datagram",
    channelId,
    from: "server",
    lifetime: "persistent",
    events: ["Update"],
    requests: {},
  } as const satisfies DatagramChannelMeta;
}

const codec = JsonCodec.shared as JsonCodec<ChannelPayload>;

describe("DatagramChannel E2E over mock connection", () => {
  it("events() receives datagrams demuxed by channel_id", async () => {
    const { client, server } = MockConnection.pair();
    const dispatcher = new DatagramDispatcher(client);
    const chan = new DatagramChannelImpl(datagramMeta("metric", 1), client, dispatcher, codec);

    const received: ChannelPayload[] = [];
    const consume = (async () => {
      for await (const e of chan.events()) {
        received.push(e);
        if (received.length === 2) break;
      }
    })();

    // server → client: [varint channelId=1][json payload]
    await sendChannelDatagram(server, 1, { name: "cpu", value: 42 });
    await sendChannelDatagram(server, 1, { name: "mem", value: 7 });
    await consume;

    expect(received).toEqual([
      { name: "cpu", value: 42 },
      { name: "mem", value: 7 },
    ]);
    await chan.close();
    await client.close();
  });

  it("fans out datagrams to multiple channels by channel_id", async () => {
    const { client, server } = MockConnection.pair();
    const dispatcher = new DatagramDispatcher(client);
    const metric = new DatagramChannelImpl(datagramMeta("metric", 1), client, dispatcher, codec);
    const agent = new DatagramChannelImpl(datagramMeta("agent", 2), client, dispatcher, codec);

    expect(dispatcher.handlerCount).toBe(2);

    const metricGot: ChannelPayload[] = [];
    const agentGot: ChannelPayload[] = [];
    const m = (async () => {
      for await (const e of metric.events()) {
        metricGot.push(e);
        break;
      }
    })();
    const a = (async () => {
      for await (const e of agent.events()) {
        agentGot.push(e);
        break;
      }
    })();

    await sendChannelDatagram(server, 2, { agent_id: "w1", status: "done" });
    await sendChannelDatagram(server, 1, { name: "cpu", value: 99 });
    await Promise.all([m, a]);

    expect(metricGot).toEqual([{ name: "cpu", value: 99 }]);
    expect(agentGot).toEqual([{ agent_id: "w1", status: "done" }]);
    await metric.close();
    await agent.close();
    await client.close();
  });

  it("sendEvent() emits a [varint channelId][payload] datagram", async () => {
    const { client, server } = MockConnection.pair();
    const dispatcher = new DatagramDispatcher(client);
    const chan = new DatagramChannelImpl(datagramMeta("metric", 5), client, dispatcher, codec);

    const serverGot: Uint8Array[] = [];
    const drain = (async () => {
      for await (const dg of server.datagrams()) {
        serverGot.push(dg);
        break;
      }
    })();
    await chan.sendEvent("Update", { name: "build", value: 1 });
    await drain;

    // 先頭 varint = channelId 5、 残りが JSON payload
    expect(serverGot[0]?.[0]).toBe(5);
    const json = new TextDecoder().decode(serverGot[0]?.subarray(1));
    expect(JSON.parse(json)).toEqual({ name: "build", value: 1 });
    await chan.close();
    await client.close();
  });

  it("close() unregisters the channel and ends events()", async () => {
    const { client } = MockConnection.pair();
    const dispatcher = new DatagramDispatcher(client);
    const chan = new DatagramChannelImpl(datagramMeta("metric", 1), client, dispatcher, codec);
    expect(dispatcher.handlerCount).toBe(1);

    await chan.close();
    expect(dispatcher.handlerCount).toBe(0);
    // events() iterator は終端済み
    const it = chan.events();
    expect((await it.next()).done).toBe(true);
    await client.close();
  });
});

/** server endpoint から [varint channelId][json payload] datagram を送る */
async function sendChannelDatagram(
  conn: MockConnection,
  channelId: number,
  payload: Record<string, unknown>,
): Promise<void> {
  const body = new TextEncoder().encode(JSON.stringify(payload));
  const buf = new Uint8Array(1 + body.length);
  buf[0] = channelId; // channelId < 128 は 1 byte varint
  buf.set(body, 1);
  await conn.sendDatagram(buf);
}
