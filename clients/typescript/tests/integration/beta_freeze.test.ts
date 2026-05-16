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
import { MockConnection, MockTransport } from "./mock_transport.js";
import { StreamServerStub } from "./server_stub.js";

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
// Blocker 3: openChannel() open-frame send + torn-down signal
//
// Phase 6b: Rust server は open ack (= open_ack) を返さない設計のため、
// `openChannel` は `__channel:` open frame の送信完了で resolve する
// (= optimistic、 Rust `client.rs::open_channel` と同形)。 明示 ack は
// Phase 6c で server 側に入る予定。 ここでは「open frame を流せた」/
// 「accept 前に stream が落ちたら reject」の 2 ケースを検証する。
//
// connect は identity handshake を待つ (= 本 test の mock server は identity を
// 送らない) ため `awaitIdentity: false` で skip する。
// ============================================================

describe("Blocker 3: openChannel open-frame + torn-down signal", () => {
  it("resolves once the __channel open frame is written", async () => {
    const transport = new MockTransport();
    const { server } = transport.prepare();
    const client = await connect({
      url: "https://x.invalid",
      transport,
      awaitIdentity: false,
    });

    // server 側: stream を accept して StreamServerStub を立てる
    const serverSide = (async () => {
      const accepted = await server.acceptStream();
      if (accepted.done) throw new Error("no stream");
      return new StreamServerStub(accepted.value);
    })();

    const channel = await client.openChannel(ControlMeta);
    expect(channel.name).toBe("control");

    const stub = await serverSide;
    // server stub は `__channel:control` open probe を観測しているはず
    await vi.waitFor(() => expect(stub.openedChannel).toBe("control"));

    await channel.close();
    await stub.close();
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
});
