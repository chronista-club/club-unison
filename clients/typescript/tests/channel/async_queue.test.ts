import { describe, expect, it } from "vitest";
import { AsyncQueue } from "../../src/channel/async_queue.js";

describe("AsyncQueue", () => {
  it("delivers buffered values in order", async () => {
    const q = new AsyncQueue<number>();
    q.push(1);
    q.push(2);
    q.end();
    const got: number[] = [];
    for await (const v of q) got.push(v);
    expect(got).toEqual([1, 2]);
  });

  it("resolves a waiting consumer when a value arrives later", async () => {
    const q = new AsyncQueue<string>();
    const next = q.next();
    q.push("late");
    expect(await next).toEqual({ value: "late", done: false });
  });

  it("ends a waiting consumer", async () => {
    const q = new AsyncQueue<string>();
    const next = q.next();
    q.end();
    expect(await next).toEqual({ value: undefined, done: true });
  });

  it("drops pushes after end", async () => {
    const q = new AsyncQueue<number>();
    q.end();
    q.push(99);
    expect(await q.next()).toEqual({ value: undefined, done: true });
  });

  it("terminates the queue when the consumer breaks", async () => {
    const q = new AsyncQueue<number>();
    q.push(1);
    for await (const _ of q) break;
    expect(await q.next()).toEqual({ value: undefined, done: true });
  });
});
