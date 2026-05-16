/**
 * エラー分類フレームワーク (Phase 5 / UNS-15)。
 *
 * boundary error を programmatic に判定可能にする。値は Rust 側の
 * `unison::ErrorCategory` (snake_case シリアライズ) と一致させること。
 * caller は `catch (e) { if (e.category === "transport") ... }` の形で分岐できる。
 */

/**
 * エラーの分類。
 *
 * - `transport`: トランスポート層 (QUIC / TLS / DNS)
 * - `protocol`: プロトコル層 (不正パケット / スキーマ不整合 / チャネル状態)
 * - `application`: アプリケーション層 (caller / handler が返したエラー)
 * - `resource`: リソース層 (quota / rate-limit / timeout)
 */
export type ErrorCategory = "transport" | "protocol" | "application" | "resource";

/** `ErrorCategory` の全値 (Rust enum の 4 variant と対応) */
export const ERROR_CATEGORIES = [
  "transport",
  "protocol",
  "application",
  "resource",
] as const satisfies readonly ErrorCategory[];
