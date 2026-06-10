/**
 * Codec ↔ channel integration test (= Phase 2e、 Medium 層)。
 *
 * channel が `JsonCodec.shared` 経由で payload を wire round-trip させ、 nested /
 * array / 各種 scalar が無損失で往復することを E2E で検証する。
 */

import { describe, expect, it } from "vitest";
import { UnisonChannelImpl } from "../../src/channel/unison_channel.js";
import type { ChannelMeta, ChannelPayload } from "../../src/channel/types.js";
import { JsonCodec } from "../../src/codec/json_codec.js";
import { MockConnection, StreamServerStub } from "../../src/testing/index.js";

const EchoMeta = {
  name: "echo",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: [],
  requests: { Echo: { request: "EchoReq", response: "EchoResp" } },
} as const satisfies ChannelMeta;

describe("Codec integration: channel over JsonCodec.shared", () => {
  it("round-trips a nested payload unchanged", async () => {
    const { client, server } = MockConnection.pair();
    const clientStream = await client.openBidiStream();
    const accepted = await server.acceptStream();
    if (accepted.done) throw new Error("no stream");
    // server は受信 payload をそのまま返す (= 真の echo)
    const stub = new StreamServerStub(accepted.value, (_m, p) => p);
    const channel = new UnisonChannelImpl(
      EchoMeta,
      clientStream,
      JsonCodec.shared as JsonCodec<ChannelPayload>,
    );

    const payload: ChannelPayload = {
      name: "vp-dashboard",
      count: 42,
      ratio: 3.14,
      enabled: true,
      tags: ["a", "b", "c"],
      meta: { nested: { deep: [1, 2, 3] }, nil: null },
    };
    const result = await channel.request("Echo", payload);
    expect(result).toEqual(payload);

    await channel.close();
    await stub.close();
  });

  it("uses JsonCodec.shared as the default codec when none is given", async () => {
    const { client, server } = MockConnection.pair();
    const clientStream = await client.openBidiStream();
    const accepted = await server.acceptStream();
    if (accepted.done) throw new Error("no stream");
    const stub = new StreamServerStub(accepted.value, (_m, p) => p);
    // codec 引数を省略 → defaultCodec (= JsonCodec.shared)
    const channel = new UnisonChannelImpl(EchoMeta, clientStream);
    const result = await channel.request("Echo", { ok: true, value: "json" });
    expect(result).toEqual({ ok: true, value: "json" });
    await channel.close();
    await stub.close();
  });
});
