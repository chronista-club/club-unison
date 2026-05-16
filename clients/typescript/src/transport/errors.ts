/**
 * Phase 2b transport layer の error 階層。
 *
 * boundary error を programmatic に判定可能にする。 全 error は
 * `UnisonTransportError` を継承し、`category` フィールド (Phase 5 / UNS-15) を
 * 持つ。 caller は `if (e.category === "transport")` の形で分岐できる。
 */

import type { ErrorCategory } from "../error/category.js";

/** transport layer error の基底 */
export class UnisonTransportError extends Error {
  /** エラー分類 (Rust 側 `unison::ErrorCategory` と値一致) */
  readonly category: ErrorCategory;

  constructor(message: string, category: ErrorCategory, options?: ErrorOptions) {
    super(message, options);
    // new.target は実際に new された subclass を指す (= 各 subclass で name 自動設定)
    this.name = new.target.name;
    this.category = category;
  }
}

/** WebTransport API が実行環境に存在しない (= 非対応 browser / 旧 Node) */
export class WebTransportUnsupportedError extends UnisonTransportError {
  constructor() {
    super("WebTransport is not available in this environment", "transport");
  }
}
