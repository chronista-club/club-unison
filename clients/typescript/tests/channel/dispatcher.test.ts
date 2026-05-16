import { describe, expect, it } from "vitest";
import { type DatagramSink, DispatcherInner } from "../../src/channel/dispatcher.js";
import { encodeVarint } from "../../src/channel/varint.js";

/** test 用 sink — push 済 payload を配列に貯める */
function recordingSink(): DatagramSink & { received: Uint8Array[]; ended: boolean } {
  const received: Uint8Array[] = [];
  return {
    received,
    ended: false,
    push(p) {
      received.push(p);
    },
    end() {
      this.ended = true;
    },
  };
}

/** [varint channelId][payload] datagram を組み立て */
function datagram(channelId: number, payload: string): Uint8Array {
  const prefix = encodeVarint(channelId);
  const body = new TextEncoder().encode(payload);
  const out = new Uint8Array(prefix.length + body.length);
  out.set(prefix, 0);
  out.set(body, prefix.length);
  return out;
}

describe("DispatcherInner", () => {
  it("starts empty", () => {
    expect(new DispatcherInner().handlerCount).toBe(0);
  });

  it("registers and unregisters handlers", () => {
    const inner = new DispatcherInner();
    inner.register(1, recordingSink());
    inner.register(2, recordingSink());
    expect(inner.handlerCount).toBe(2);
    inner.unregister(1);
    expect(inner.handlerCount).toBe(1);
  });

  it("ends the old sink when a channelId is re-registered", () => {
    const inner = new DispatcherInner();
    const first = recordingSink();
    inner.register(7, first);
    inner.register(7, recordingSink());
    expect(first.ended).toBe(true);
    expect(inner.handlerCount).toBe(1);
  });

  it("routes datagrams to the registered channel by channel_id", () => {
    const inner = new DispatcherInner();
    const s1 = recordingSink();
    const s2 = recordingSink();
    inner.register(1, s1);
    inner.register(2, s2);
    inner.dispatch(datagram(1, "for-1"));
    inner.dispatch(datagram(2, "for-2"));
    expect(new TextDecoder().decode(s1.received[0])).toBe("for-1");
    expect(new TextDecoder().decode(s2.received[0])).toBe("for-2");
  });

  it("silently drops datagrams for unknown channel_id", () => {
    const inner = new DispatcherInner();
    expect(() => inner.dispatch(datagram(99, "orphan"))).not.toThrow();
  });

  it("silently drops malformed varint datagrams", () => {
    const inner = new DispatcherInner();
    const sink = recordingSink();
    inner.register(1, sink);
    inner.dispatch(new Uint8Array(11).fill(0xff));
    expect(sink.received).toHaveLength(0);
  });

  it("ends every sink on clear", () => {
    const inner = new DispatcherInner();
    const sink = recordingSink();
    inner.register(1, sink);
    inner.clear();
    expect(sink.ended).toBe(true);
    expect(inner.handlerCount).toBe(0);
  });
});
