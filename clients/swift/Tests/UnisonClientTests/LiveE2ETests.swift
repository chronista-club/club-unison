import Foundation
import Testing
@testable import UnisonClient

// live e2e: 実 NWProtocolQUIC ↔ quinn (`unison mock`)。
//
// CI には mock server が無いため、 環境変数 UNISON_LIVE=1 のときだけ実行する。
//   1) cd .. && target/debug/unison mock --schema schemas/ping_pong.kdl --addr '[::1]:7878'
//   2) UNISON_LIVE=1 swift test --filter LiveE2E
//
// これが通れば、 connect → openChannel → request → response decode の全層が
// 実 QUIC 上で Rust server と interop していることの最終証明になる。

private struct PingPongMeta: StreamChannelMeta {
    static let name = "ping-pong"
    struct Tick: Codable, Sendable {}
    typealias Event = Tick
}

private struct Ping: UnisonRequest {
    static let method = "Ping"
    struct Pong: Codable, Sendable { let reply: String; let timestamp: String? }
    typealias Response = Pong
    let message: String
}

struct LiveE2ETests {
    private static var enabled: Bool {
        ProcessInfo.processInfo.environment["UNISON_LIVE"] == "1"
    }

    @Test(.enabled(if: LiveE2ETests.enabled))
    func liveRequestRoundTripAgainstMock() async throws {
        let conn = try await UnisonClient.connect(to: .localDaemon(port: 7878), trust: .skipVerify)
        let channel = try await conn.openChannel(PingPongMeta())
        let pong = try await channel.request(Ping(message: "hi"))
        // mock は returns 型から stub 値を返す (string → "")。 round-trip 成立が要点。
        #expect(pong.reply == "")
        await channel.close()
        await conn.disconnect()
    }
}
