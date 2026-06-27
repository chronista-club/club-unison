import Foundation
import Testing
@testable import UnisonClient

// Connection-level auth (= v1.4.0) の in-memory テスト。
// - credential が JSON 数値配列で encode される (= Rust Vec<u8> 互換、 Data の base64 化回避)
// - authenticate の ok/deny flow (= unison.auth open → Authenticate → AuthResult)
// 設計: design/connection-auth.md §5.8。

struct AuthTests {
    @Test func credentialEncodesAsByteArray() throws {
        let req = AuthenticateRequest(credential: [104, 101, 108, 108, 111])
        let data = try JSONEncoder().encode(req)
        let json = String(decoding: data, as: UTF8.self)
        // credential は JSON 数値配列でなければならない (= Rust Vec<u8> 互換)。
        // Data 型にすると JSONEncoder が base64 string 化して非互換になる。
        #expect(json == #"{"credential":[104,101,108,108,111]}"#)
    }

    @Test func authenticateSucceedsOnOk() async throws {
        let transport = StubTransport(respond: { method, _ in
            method == authenticateMethod ? Data(#"{"ok":true}"#.utf8) : nil
        })
        let conn = Connection(transport: transport)
        // ok=true なら throw しない。
        try await conn.authenticate([1, 2, 3])
    }

    @Test func authenticateThrowsOnDeny() async throws {
        let transport = StubTransport(respond: { method, _ in
            method == authenticateMethod ? Data(#"{"ok":false}"#.utf8) : nil
        })
        let conn = Connection(transport: transport)
        do {
            try await conn.authenticate([9, 9, 9])
            Issue.record("ok=false なら authenticationDenied を throw すべき")
        } catch UnisonError.authenticationDenied {
            // 期待どおり
        }
    }
}
