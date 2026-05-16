/**
 * VP dashboard ergonomics demo (= v1.0 Phase 3b、 first proof point)。
 *
 * design `typescript-client-api.md` §2 の Vantage Point dashboard use case を
 * 「これで dashboard 書きたい」 ideal caller code として書き起こし、 in-TS mock
 * transport で実際に走らせる。 走らせると metric / agent-status の dashboard
 * 更新が stdout に出力される。
 *
 * 構成:
 * - PART A: ideal caller code (= dashboard dev が WANT to write するコード)
 * - PART B: mock server harness (= 実 unison server の代役、 demo を self-contained に)
 *
 * 実行: `npm run example`
 */

import { connectClient, type ChannelMeta, type DatagramChannelMeta } from "../src/index.js";
import { JsonCodec } from "../src/codec/json_codec.js";
import { encodeVarint } from "../src/channel/varint.js";
import { MockTransport, type MockConnection } from "../tests/integration/mock_transport.js";
import { StreamServerStub } from "../tests/integration/server_stub.js";
import type { BidiStream } from "../src/transport/types.js";

// ============================================================
// Channel meta (= Phase 1 codegen 出力の代用、 vp-dashboard KDL schema 由来)
// ============================================================

/** Datagram metric broadcast (= 60Hz refresh、 channel_id=1) */
const MetricChannelMeta = {
  name: "metric",
  backend: "datagram",
  channelId: 1,
  from: "server",
  lifetime: "persistent",
  events: ["MetricUpdate"],
  requests: {},
} as const satisfies DatagramChannelMeta;

/** Agent status (= less frequent、 stream channel で reliable) */
const AgentStatusChannelMeta = {
  name: "agent_status",
  backend: "stream",
  from: "server",
  lifetime: "persistent",
  events: ["AgentEvent"],
  requests: {},
} as const satisfies ChannelMeta;

/** Dashboard control (= client → server、 request/response) */
const ControlChannelMeta = {
  name: "control",
  backend: "stream",
  from: "client",
  lifetime: "persistent",
  events: [],
  requests: {
    SubscribeMetric: { request: "SubscribeMetricReq", response: "Subscribed" },
  },
} as const satisfies ChannelMeta;

// ============================================================
// PART A: ideal caller code (= VP dashboard dev が書きたいコード)
// ============================================================

/** Canvas dashboard が持つ metric store の最小版 */
const dashboardStore = new Map<string, number>();

async function runDashboard(transport: MockTransport): Promise<void> {
  // --- Connection setup ---
  const client = await connectClient({
    url: "https://vp.chronista.local:8080",
    trust: "system",
    transport, // 本番では省略 (= WebTransport default)
  });

  // Connection lifecycle を監視 (= 自前 reconnect の起点、 library は auto-reconnect しない)
  void (async () => {
    for await (const ev of client.events()) {
      if (ev.type === "connected") console.log(`[conn] connected: ${ev.remoteAddr}`);
      else if (ev.type === "disconnected") console.warn(`[conn] disconnected: ${ev.reason}`);
    }
  })();

  // --- Control channel: subscribe を request/response で要求 ---
  const control = await client.openChannel(ControlChannelMeta);
  const subscribed = await control.request("SubscribeMetric", {
    names: ["cpu", "memory", "build_progress"],
  });
  console.log(`[control] SubscribeMetric -> ok=${String(subscribed["ok"])}`);

  // --- Datagram metric channel: 60Hz の steady stream を subscribe ---
  const metricChan = client.openDatagramChannel(MetricChannelMeta);
  const metricLoop = (async () => {
    for await (const update of metricChan.events()) {
      // FRICTION: update は ChannelPayload (= Record<string,unknown>)、 MetricUpdate に narrow されない
      dashboardStore.set(String(update["name"]), Number(update["value"]));
      console.log(`[metric] ${String(update["name"])} = ${String(update["value"])}`);
    }
  })();

  // --- Agent status channel: stream で reliable な event subscribe ---
  const agentChan = await client.openChannel(AgentStatusChannelMeta);
  const agentLoop = (async () => {
    for await (const ev of agentChan.events()) {
      console.log(`[agent] ${String(ev["agent_id"])} -> ${String(ev["status"])}`);
    }
  })();

  // demo: server harness が一定数 push したら teardown する
  await new Promise((r) => setTimeout(r, 200));

  // --- Cleanup ---
  await metricChan.close();
  await agentChan.close();
  await control.close();
  await client.disconnect("demo complete");
  await Promise.all([metricLoop, agentLoop]);

  console.log(`[done] dashboard store: ${JSON.stringify([...dashboardStore])}`);
}

// ============================================================
// PART B: mock server harness (= 実 unison server の代役)
// ============================================================

const codec = JsonCodec.shared;

/** server endpoint から [varint channelId][json payload] datagram を流す */
async function pushMetric(server: MockConnection, name: string, value: number): Promise<void> {
  const prefix = encodeVarint(MetricChannelMeta.channelId);
  const body = codec.encode({ name, value, unit: "%" });
  const buf = new Uint8Array(prefix.length + body.length);
  buf.set(prefix, 0);
  buf.set(body, prefix.length);
  await server.sendDatagram(buf);
}

/** server endpoint で client が open した bidi stream を 1 本 accept */
async function acceptStream(server: MockConnection): Promise<BidiStream> {
  const accepted = await server.acceptStream();
  if (accepted.done) throw new Error("server did not receive stream");
  return accepted.value;
}

async function runServer(server: MockConnection): Promise<void> {
  // control channel (request/response)、 agent_status channel (event push) を accept
  const controlStub = new StreamServerStub(await acceptStream(server), (method) => {
    if (method === "SubscribeMetric") return { ok: true };
    return { ok: false };
  });
  const agentStub = new StreamServerStub(await acceptStream(server));

  // datagram metric を数発 push
  for (const [name, value] of [
    ["cpu", 42],
    ["memory", 71],
    ["build_progress", 88],
  ] as const) {
    await pushMetric(server, name, value);
  }
  // agent status を push
  await agentStub.pushEvent("AgentEvent", { agent_id: "w1", status: "running" });
  await agentStub.pushEvent("AgentEvent", { agent_id: "w1", status: "completed" });

  await new Promise((r) => setTimeout(r, 150));
  await controlStub.close();
  await agentStub.close();
}

// ============================================================
// Demo entry
// ============================================================

async function main(): Promise<void> {
  const transport = new MockTransport();
  const { server } = transport.prepare(); // client は connectClient() が払い出す
  await Promise.all([runDashboard(transport), runServer(server)]);
}

main().catch((err: unknown) => {
  console.error("demo failed:", err);
  process.exitCode = 1;
});
