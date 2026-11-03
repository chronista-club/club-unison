/**
 * `check` が throw しなくなるまで待つ (= vitest の `vi.waitFor` 相当)。
 *
 * `bun:test` には waitFor が無いので、 mock transport 越しの非同期な観測
 * (= server stub が frame を処理し終えるのを待つ) 用に置く。 既定値は
 * vitest と揃える (timeout 1000ms / interval 50ms)。
 */
export async function waitFor(
  check: () => void,
  { timeout = 1000, interval = 50 }: { timeout?: number; interval?: number } = {},
): Promise<void> {
  const deadline = Date.now() + timeout;
  for (;;) {
    try {
      check();
      return;
    } catch (error) {
      if (Date.now() >= deadline) throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
}
