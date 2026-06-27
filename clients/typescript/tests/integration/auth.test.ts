/**
 * Connection-level auth (= v1.4.0) test。
 *
 * - `toAuthenticateRequest` の wire 互換 (= credential を number[] で送る、 Rust Vec<u8> 互換)
 * - `UnisonClient.authenticate` の full flow (= unison.auth open → Authenticate → ok/deny)
 *   を MockConnection + StreamServerStub で検証。
 *
 * 設計: `design/connection-auth.md` §5.8。
 */

import { describe, expect, it } from "vitest";
import { UnisonClient } from "../../src/client.js";
import {
  AUTHENTICATE_METHOD,
  toAuthenticateRequest,
} from "../../src/channel/auth.js";
import { MockConnection, StreamServerStub } from "../../src/testing/index.js";

describe("auth: toAuthenticateRequest wire encoding", () => {
  it("credential を number[] で encode する (= Rust Vec<u8> 互換)", () => {
    const req = toAuthenticateRequest(new Uint8Array([104, 101, 108, 108, 111]));
    expect(Array.isArray(req.credential)).toBe(true);
    expect(req.credential).toEqual([104, 101, 108, 108, 111]);
    // JSON は数値配列でなければならない (= Uint8Array 直渡しの {"0":..} object は非互換)
    expect(JSON.stringify(req)).toBe('{"credential":[104,101,108,108,111]}');
  });

  it("空 credential も配列になる", () => {
    expect(toAuthenticateRequest(new Uint8Array([])).credential).toEqual([]);
  });
});

describe("auth: UnisonClient.authenticate over mock connection", () => {
  it("正当 credential → ok=true で resolve、 credential が number[] で server に届く", async () => {
    const { client: clientConn, server: serverConn } = MockConnection.pair();
    const client = new UnisonClient(clientConn);

    let receivedMethod: string | undefined;
    let receivedCredential: unknown;
    const serverReady = (async () => {
      const accepted = await serverConn.acceptStream();
      if (accepted.done) throw new Error("no stream accepted");
      return new StreamServerStub(accepted.value, (method, payload) => {
        receivedMethod = method;
        receivedCredential = payload.credential;
        return { ok: true };
      });
    })();

    await client.authenticate(new Uint8Array([1, 2, 3]));
    const stub = await serverReady;

    expect(receivedMethod).toBe(AUTHENTICATE_METHOD);
    expect(receivedCredential).toEqual([1, 2, 3]);

    await stub.close();
    await client.disconnect();
  });

  it("不正 credential → ok=false で throw する", async () => {
    const { client: clientConn, server: serverConn } = MockConnection.pair();
    const client = new UnisonClient(clientConn);

    const serverReady = (async () => {
      const accepted = await serverConn.acceptStream();
      if (accepted.done) throw new Error("no stream accepted");
      return new StreamServerStub(accepted.value, () => ({ ok: false }));
    })();

    await expect(
      client.authenticate(new Uint8Array([9, 9, 9])),
    ).rejects.toThrow(/denied/);

    const stub = await serverReady;
    await stub.close();
    await client.disconnect();
  });
});
