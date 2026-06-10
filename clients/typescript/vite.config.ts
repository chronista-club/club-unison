import { defineConfig } from "vite";
import { resolve } from "node:path";

/**
 * Vite config for @chronista-club/unison-client TS SDK (= v1.0 polyglot Phase 2)
 *
 * - Library build mode (= index + testing の 2 entry、 dist/ に ESM 出力)
 * - Type definitions は別途 `tsc --emitDeclarationOnly` で生成 (= package.json build script)
 * - Bundle size 目標: core ≤ 100KB / +Proto ≤ 200KB minified gzipped
 */
export default defineConfig({
  build: {
    lib: {
      entry: {
        index: resolve(import.meta.dirname, "src/index.ts"),
        // `/testing` subpath (= dogfood signal #1、 mock harness 公式配布)
        testing: resolve(import.meta.dirname, "src/testing/index.ts"),
      },
      name: "UnisonClient",
      formats: ["es"],
      fileName: (_format, entryName) => `${entryName}.js`,
    },
    sourcemap: true,
    minify: "esbuild",
    target: "es2022",
    rollupOptions: {
      // Future: @bufbuild/protobuf は peer dep or external 化検討 (= Phase 2d で確定)
      external: [],
    },
  },
  resolve: {
    alias: {
      "@": resolve(import.meta.dirname, "src"),
    },
  },
});
