/**
 * UnisonChannel E2E integration test (= Phase 2e、 t-wada pyramid の Medium 層)。
 *
 * mock bidi stream 上で `request()` round-trip / `events()` 受信 / `sendEvent()` を
 * 実 transport なしで検証する。
 */

import { describe, expect, it, vi } from "vitest";
import { UnisonChannelImpl } from "../../src/channel/unison_channel.js";
import type { ChannelMeta } from "../../src/channel/types.js";
import { JsonCodec } from "../../src/codec/json_codec.js";
import type { ChannelPayload } from "../../src/channel/types.js";
import { MockConnection, StreamServerStub } from "../../src/testing/index.js";

const ControlMeta = {
  name: "control",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: ["Notice"],
  requests: {
    SubscribeMetric: { request: "SubscribeMetricReq", response: "Subscribed" },
  },
} as const satisfies ChannelMeta;

const codec = JsonCodec.shared as JsonCodec<ChannelPayload>;

/** client/server connection ペアを用意し、 server stub 付き channel を開設 */
async function openChannelPair(
  handler?: ConstructorParameters<typeof StreamServerStub>[1],
) {
  const { client, server } = MockConnection.pair();
  const clientStream = await client.openBidiStream();
  const accepted = await server.acceptStream();
  if (accepted.done) throw new Error("server did not receive stream");
  const stub = new StreamServerStub(accepted.value, handler);
  const channel = new UnisonChannelImpl(ControlMeta, clientStream, codec);
  return { client, server, channel, stub };
}

describe("UnisonChannel E2E over mock connection", () => {
  it("request() round-trips a payload through codec", async () => {
    const { channel, stub } = await openChannelPair((method, payload) => {
      expect(method).toBe("SubscribeMetric");
      expect(payload).toEqual({ names: ["cpu", "memory"] });
      return { ok: true };
    });
    const result = await channel.request("SubscribeMetric", {
      names: ["cpu", "memory"],
    });
    expect(result).toEqual({ ok: true });
    await channel.close();
    await stub.close();
  });

  it("request() rejects on an error frame from the server", async () => {
    const { channel, stub } = await openChannelPair();
    // server 側で error frame を返すため、 request id=1 宛てに error を push
    const reqPromise = channel.request("SubscribeMetric", { names: [] });
    await stub.pushError(1, "SubscribeMetric", { reason: "denied" });
    await expect(reqPromise).rejects.toThrow(/denied/);
    await channel.close();
    await stub.close();
  });

  it("events() receives server-pushed events as an AsyncIterable", async () => {
    const { channel, stub } = await openChannelPair();
    const received: ChannelPayload[] = [];
    const consume = (async () => {
      for await (const e of channel.events()) {
        received.push(e);
        if (received.length === 2) break;
      }
    })();
    await stub.pushEvent("Notice", { msg: "first" });
    await stub.pushEvent("Notice", { msg: "second" });
    await consume;
    expect(received).toEqual([{ msg: "first" }, { msg: "second" }]);
    await channel.close();
    await stub.close();
  });

  it("sendEvent() delivers a client→server event", async () => {
    const { channel, stub } = await openChannelPair();
    await channel.sendEvent("Notice", { msg: "hello-server" });
    // server stub の recv loop が処理するまで待つ
    await vi.waitFor(() => expect(stub.receivedEvents).toHaveLength(1));
    expect(stub.receivedEvents[0]).toEqual({
      method: "Notice",
      payload: { msg: "hello-server" },
    });
    await channel.close();
    await stub.close();
  });

  it("close() fails in-flight requests", async () => {
    const { channel, stub } = await openChannelPair();
    const pending = channel.request("SubscribeMetric", { names: [] });
    await stub.close(); // server 側 stream close → client recv loop 終端
    await expect(pending).rejects.toThrow(/closed/);
    await channel.close();
  });
});
