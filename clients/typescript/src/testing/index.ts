/**
 * `@chronista-club/unison-client/testing` — 公式 mock harness subpath
 *
 * 実 WebTransport / Rust server なしで SDK を E2E 駆動するための test 基盤。
 * dogfood signal #1 (= `dogfood/vp-2026-05-26.md`) で「範の写経に 6 file copy が
 * 要る」friction が報告され、`npm install` + import 1 行に圧縮するために公開した。
 *
 * - `MockTransport` / `MockConnection`: in-memory `Transport` / `Connection` 実装
 * - `StreamServerStub`: Rust `quic.rs` と byte 互換の frame echo server
 * - `TEST_IDENTITY` / `sendIdentity`: identity handshake fixture
 *
 * SDK 本体の integration test (`tests/integration/`) もここを import する
 * (= 配布物と test 基盤が同一 source、 乖離しない)。
 */

export {
	makeBidiPair,
	MockConnection,
	MockTransport,
} from "./mock_transport.js";
export type {
	ReceivedEvent,
	RequestHandler,
	StreamServerStubOptions,
} from "./server_stub.js";
export {
	sendIdentity,
	StreamServerStub,
	TEST_IDENTITY,
} from "./server_stub.js";
