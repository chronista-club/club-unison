/**
 * Phase 2b transport layer の error 階層。
 *
 * boundary error を programmatic に判定可能にする (= Phase 5 の ErrorCategory
 * framework の前身)。 全 error は `UnisonTransportError` を継承する。
 */

/** transport layer error の基底 */
export class UnisonTransportError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    // new.target は実際に new された subclass を指す (= 各 subclass で name 自動設定)
    this.name = new.target.name;
  }
}

/** WebTransport API が実行環境に存在しない (= 非対応 browser / 旧 Node) */
export class WebTransportUnsupportedError extends UnisonTransportError {
  constructor() {
    super("WebTransport is not available in this environment");
  }
}
