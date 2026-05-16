/**
 * AsyncQueue (= Phase 2c 内部 util)。
 *
 * push 駆動の値を `for await` で pull できる単一消費者 queue。 channel の
 * `events()` が server push / datagram demux の payload を AsyncIterable に
 * 橋渡しするのに使う。 producer は非同期境界なしで `push` / `end` を呼べる。
 */

/** push された値を AsyncIterable として配る単一消費者 queue */
export class AsyncQueue<T> implements AsyncIterableIterator<T> {
  readonly #buffer: T[] = [];
  /** 値待ちで suspend した consumer の resolver */
  #pending: ((r: IteratorResult<T>) => void) | undefined;
  #closed = false;

  /** 値を 1 件投入 (= 待機中 consumer があれば即配送) */
  push(value: T): void {
    if (this.#closed) return;
    if (this.#pending !== undefined) {
      const resolve = this.#pending;
      this.#pending = undefined;
      resolve({ value, done: false });
    } else {
      this.#buffer.push(value);
    }
  }

  /** queue を終端 (= 以降の `next()` は done、 待機中 consumer を解放) */
  end(): void {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#pending !== undefined) {
      const resolve = this.#pending;
      this.#pending = undefined;
      resolve({ value: undefined, done: true });
    }
  }

  next(): Promise<IteratorResult<T>> {
    const buffered = this.#buffer.shift();
    if (buffered !== undefined) {
      return Promise.resolve({ value: buffered, done: false });
    }
    if (this.#closed) {
      return Promise.resolve({ value: undefined, done: true });
    }
    return new Promise((resolve) => {
      this.#pending = resolve;
    });
  }

  /** consumer が `break` した時の cleanup hook (= queue 終端) */
  return(): Promise<IteratorResult<T>> {
    this.end();
    return Promise.resolve({ value: undefined, done: true });
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<T> {
    return this;
  }
}
