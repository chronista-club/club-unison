/**
 * beta-API-freeze blocker regression tests (= Phase 3b 由来の 3 件)。
 *
 * - Blocker 1: payload type narrowing — `events()` / `request()` が `__types`
 *   carrier 経由で生成 interface に narrow する (= `Record<string, unknown>` ではない)
 * - Blocker 3: `openChannel()` の no-accept signal — server peer が bidi stream を
 *   accept しなければ reject する
 */

import { describe, expect, it, vi } from "vitest";
import { connect } from "../../src/client.js";
import { DatagramChannelImpl } from "../../src/channel/datagram_channel.js";
import { DatagramDispatcher } from "../../src/channel/dispatcher.js";
import type {
  ChannelMeta,
  ChannelPayload,
  DatagramChannelMeta,
  EventType,
  RequestType,
  ResponseType,
} from "../../src/channel/types.js";
import { JsonCodec } from "../../src/codec/json_codec.js";
import {
  MockConnection,
  MockTransport,
  StreamServerStub,
} from "../../src/testing/index.js";

// --- 生成 meta の代用 (= codegen 出力相当、 `__types` phantom carrier 込み) ---

interface MetricUpdate {
  name: string;
  value: number;
  unit?: string;
}
interface SubReq {
  topic: string;
}
interface SubResp {
  ok: boolean;
}

const MetricMeta = {
  name: "metric",
  backend: "datagram",
  channelId: 1,
  from: "server",
  lifetime: "persistent",
  events: ["MetricUpdate"],
  requests: {},
  __types: undefined as unknown as {
    events: { MetricUpdate: MetricUpdate };
    requests: Record<string, never>;
  },
} as const satisfies DatagramChannelMeta;

const ControlMeta = {
  name: "control",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: [],
  requests: { Sub: { request: "SubReq", response: "SubResp" } },
  __types: undefined as unknown as {
    events: Record<string, never>;
    requests: { Sub: { request: SubReq; response: SubResp } };
  },
} as const satisfies ChannelMeta;

const codec = JsonCodec.shared as JsonCodec<ChannelPayload>;

// ============================================================
// Blocker 1: payload type narrowing (= 型レベル assertion)
// ============================================================

/** `Equals` helper — `T` と `U` が厳密に一致すれば `true` */
type Equals<T, U> =
  (<G>() => G extends T ? 1 : 2) extends <G>() => G extends U ? 1 : 2
    ? true
    : false;

/** compile-time に成立しなければ tsc が fail する型レベル assertion */
function assertType<T extends true>(): void {
  void (0 as unknown as T);
}

describe("Blocker 1: payload type narrowing via __types carrier", () => {
  it("EventType<M> resolves to the generated interface, not Record<string, unknown>", () => {
    // datagram channel: events() の要素型 = MetricUpdate
    assertType<Equals<EventType<typeof MetricMeta>, MetricUpdate>>();
    // generated interface であって ChannelPayload (= Record) ではない
    assertType<Equals<EventType<typeof MetricMeta>, ChannelPayload> extends true ? false : true>();
  });

  it("RequestType / ResponseType resolve to the generated interfaces", () => {
    assertType<Equals<RequestType<typeof ControlMeta, "Sub">, SubReq>>();
    assertType<Equals<ResponseType<typeof ControlMeta, "Sub">, SubResp>>();
  });

  it("events() yields the generated interface at runtime", async () => {
    const { client, server } = MockConnection.pair();
    const dispatcher = new DatagramDispatcher(client);
    const chan = new DatagramChannelImpl(MetricMeta, client, dispatcher, codec);

    const consume = (async () => {
      for await (const update of chan.events()) {
        // update は MetricUpdate に narrow — .name / .value が手動 cast なしで typed
        const name: string = update.name;
        const value: number = update.value;
        return { name, value };
      }
      return undefined;
    })();

    const body = new TextEncoder().encode(
      JSON.stringify({ name: "cpu", value: 42, unit: "%" }),
    );
    const dg = new Uint8Array(1 + body.length);
    dg[0] = 1; // channelId varint
    dg.set(body, 1);
    await server.sendDatagram(dg);

    expect(await consume).toEqual({ name: "cpu", value: 42 });
    await chan.close();
    await client.close();
  });
});

// ============================================================
// Blocker 3: openChannel() の real accept signal (= Phase 6c)
//
// Phase 6c: Rust server は open frame に対し同 stream へ `open_ack`
// (= method `__channel_ack`、 open request と同 id) を返す。 `openChannel` は
// この ack を await する (= optimistic-resolve 廃止):
//   - Response の open_ack → resolve
//   - Error の nack (= channel-not-found) → reject
//   - ack が来ない (= peer 不在 / stream tear-down / timeout) → reject
//
// connect は identity handshake を待つ (= 本 test の mock server は identity を
// 送らない) ため `awaitIdentity: false` で skip する。
// ============================================================

describe("Blocker 3: openChannel real accept signal (open_ack)", () => {
  it("resolves once the server replies with an open_ack", async () => {
    const transport = new MockTransport();
    const { server } = transport.prepare();
    const client = await connect({
      url: "https://x.invalid",
      transport,
      awaitIdentity: false,
    });

    // server 側: stream を accept して StreamServerStub を立てる
    // (= stub の constructor が open frame を受けて open_ack を返す)
    const serverSide = (async () => {
      const accepted = await server.acceptStream();
      if (accepted.done) throw new Error("no stream");
      return new StreamServerStub(accepted.value);
    })();

    const [channel, stub] = await Promise.all([
      client.openChannel(ControlMeta),
      serverSide,
    ]);
    expect(channel.name).toBe("control");
    // server stub は `__channel:control` open probe を観測しているはず
    await vi.waitFor(() => expect(stub.openedChannel).toBe("control"));

    await channel.close();
    await stub.close();
    await client.disconnect();
  });

  it("rejects when the server sends a channel-not-found nack", async () => {
    const transport = new MockTransport();
    const { server } = transport.prepare();
    const client = await connect({
      url: "https://x.invalid",
      transport,
      awaitIdentity: false,
    });

    // server stub は open frame に対し nack (= Error) を返す
    const serverSide = (async () => {
      const accepted = await server.acceptStream();
      if (accepted.done) throw new Error("no stream");
      return new StreamServerStub(accepted.value, undefined, {
        rejectOpen: true,
      });
    })();

    await expect(
      Promise.all([client.openChannel(ControlMeta), serverSide]),
    ).rejects.toThrow(/channel-not-found/);
    await client.disconnect();
  });

  it("rejects when the stream is torn down before the open frame", async () => {
    const transport = new MockTransport();
    const { server } = transport.prepare();
    const client = await connect({
      url: "https://x.invalid",
      transport,
      awaitIdentity: false,
    });

    await server.close("server gone"); // open 前に connection drop
    await expect(client.openChannel(ControlMeta)).rejects.toThrow(
      /could not be opened|closed before it was accepted|connection closed/,
    );
    await client.disconnect();
  });

  it("rejects on timeout when no server peer ever accepts", async () => {
    const transport = new MockTransport();
    transport.prepare(); // server 側 stream を誰も accept しない
    const client = await connect({
      url: "https://x.invalid",
      transport,
      awaitIdentity: false,
    });

    // open frame は流せるが open_ack が来ない → timeout で reject
    await expect(client.openChannel(ControlMeta, 100)).rejects.toThrow(
      /was not accepted within/,
    );
    await client.disconnect();
  });
});
