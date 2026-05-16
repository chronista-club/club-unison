/**
 * Connection teardown cascade integration test (= Phase 2e、 Medium 層)。
 *
 * `Connection.close()` で disconnected event が流れ、 配下の stream / datagram
 * channel が連鎖して終端することを検証する。
 */

import { describe, expect, it } from "vitest";
import { UnisonChannelImpl } from "../../src/channel/unison_channel.js";
import { DatagramChannelImpl } from "../../src/channel/datagram_channel.js";
import { DatagramDispatcher } from "../../src/channel/dispatcher.js";
import type { ChannelMeta, DatagramChannelMeta, ChannelPayload } from "../../src/channel/types.js";
import type { ConnectionEvent } from "../../src/transport/types.js";
import { JsonCodec } from "../../src/codec/json_codec.js";
import { MockConnection } from "./mock_transport.js";
import { StreamServerStub } from "./server_stub.js";

const StreamMeta = {
  name: "ctl",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: ["E"],
  requests: { Whatever: { request: "WhateverReq", response: "WhateverResp" } },
} as const satisfies ChannelMeta;

const DgMeta = {
  name: "metric",
  backend: "datagram",
  channelId: 1,
  from: "server",
  lifetime: "persistent",
  events: ["Update"],
  requests: {},
} as const satisfies DatagramChannelMeta;

const codec = JsonCodec.shared as JsonCodec<ChannelPayload>;

describe("Connection teardown cascade", () => {
  it("emits disconnected on Connection.events() when closed", async () => {
    const { client } = MockConnection.pair();
    const events: ConnectionEvent[] = [];
    const consume = (async () => {
      for await (const e of client.events()) events.push(e);
    })();
    await client.close("bye");
    await consume;
    expect(events[0]).toEqual({ type: "connected", remoteAddr: "mock://server" });
    expect(events.at(-1)).toEqual({ type: "disconnected", reason: "bye" });
  });

  it("ends a stream channel's recv loop when the connection closes", async () => {
    const { client, server } = MockConnection.pair();
    const clientStream = await client.openBidiStream();
    const accepted = await server.acceptStream();
    if (accepted.done) throw new Error("no stream");
    const stub = new StreamServerStub(accepted.value);
    const channel = new UnisonChannelImpl(StreamMeta, clientStream, codec);

    // connection close → server stream tear down → client recv loop 終端 → events() done
    await server.close("server gone");
    const it = channel.events();
    expect((await it.next()).done).toBe(true);

    await channel.close();
    await stub.close();
  });

  it("ends a datagram channel's events() when the dispatcher loop terminates", async () => {
    const { client, server } = MockConnection.pair();
    const dispatcher = new DatagramDispatcher(client);
    const chan = new DatagramChannelImpl(DgMeta, client, dispatcher, codec);
    const it = chan.events();

    // connection close → datagrams() iterator 終端 → drain loop の finally で clear → sink.end
    await client.close("teardown");
    expect((await it.next()).done).toBe(true);
    expect(dispatcher.handlerCount).toBe(0);
    void server;
  });

  it("rejects in-flight requests when the connection drops", async () => {
    const { client, server } = MockConnection.pair();
    const clientStream = await client.openBidiStream();
    // server stub を立てない = request に応答しない (= 在中のまま connection drop)
    const channel = new UnisonChannelImpl(StreamMeta, clientStream, codec);

    const pending = channel.request("Whatever", { x: 1 });
    await server.close("drop"); // 配下 stream tear down → client recv loop 終端
    await expect(pending).rejects.toThrow(/closed/);

    await channel.close();
  });
});
