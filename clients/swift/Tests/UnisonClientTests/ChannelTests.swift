import Foundation
import Testing
@testable import UnisonClient

// channel 状態機械の in-memory テスト (= NWProtocolQUIC 抜きで open / request-response
// / event / identity の round-trip を決定論的に検証する)。

private struct PingPongMeta: StreamChannelMeta {
    static let name = "ping-pong"
    struct Tick: Codable, Sendable, Equatable { let seq: Int }
    typealias Event = Tick
}

private struct EchoReq: UnisonRequest {
    static let method = "Echo"
    struct Resp: Codable, Sendable, Equatable { let data: String }
    typealias Response = Resp
    let data: String
}

struct ChannelTests {
    @Test func openChannelSucceedsOnAck() async throws {
        let conn = Connection(transport: StubTransport())
        let channel = try await conn.openChannel(PingPongMeta())
        await channel.close()
        // open が ack され throw しなければ成功。
    }

    @Test func openChannelRejectedOnNack() async throws {
        let conn = Connection(transport: StubTransport(rejectOpen: true))
        await #expect(throws: UnisonError.self) {
            _ = try await conn.openChannel(PingPongMeta())
        }
    }

    @Test func requestEchoRoundTrip() async throws {
        let conn = Connection(transport: StubTransport())
        let channel = try await conn.openChannel(PingPongMeta())
        let resp = try await channel.request(EchoReq(data: "hello-unison"))
        #expect(resp.data == "hello-unison")
        await channel.close()
    }

    @Test func serverPushedEventArrives() async throws {
        let tick = Data(#"{"seq":7}"#.utf8)
        let conn = Connection(transport: StubTransport(pushEvents: [tick]))
        let channel = try await conn.openChannel(PingPongMeta())
        var received: PingPongMeta.Tick?
        for await event in channel.events {
            received = event
            break
        }
        #expect(received == PingPongMeta.Tick(seq: 7))
        await channel.close()
    }

    @Test func serverIdentityIsParsed() async throws {
        let conn = Connection(transport: StubTransport())
        let identity = try await conn.serverIdentity()
        #expect(identity.name == "stub")
        #expect(identity.version == "1.0.0")
        #expect(identity.channels == ["ping-pong"])
    }
}
