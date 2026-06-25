import Testing
@testable import UnisonClient

// scaffold 段階の smoke test。 transport の実 handshake は別 (要 server)。
// ここでは API surface の型と純粋ロジックが成立することを確認する。

@Test func alpnMatchesRustConstant() {
    // Rust `network::UNISON_ALPN` と一致していること (= interop の生命線)。
    #expect(QUICTransport.alpn == "unison")
}

@Test func endpointAndTrustAreValueTypes() {
    #expect(Endpoint.localDaemon(port: 7878) == .localDaemon(port: 7878))
    #expect(Endpoint.host("example.com", port: 443) != .localDaemon(port: 443))
    #expect(TrustPolicy.skipVerify == .skipVerify)
    #expect(TrustPolicy.system != .skipVerify)
}

@Test func serverIdentityHoldsChannels() {
    let id = ServerIdentity(name: "n", version: "1.0.0", namespace: "ns", channels: ["a", "b"])
    #expect(id.channels == ["a", "b"])
    #expect(id.name == "n")
}

// 例: consumer が定義する channel meta はこう書ける (= 生成 or 手書きの形)。
private struct PingPongMeta: StreamChannelMeta {
    static let name = "ping-pong"
    struct Tick: Sendable, Equatable {}
    typealias Event = Tick
}

private struct Ping: UnisonRequest {
    static let method = "Ping"
    struct Pong: Sendable, Equatable { let reply: String }
    typealias Response = Pong
    let message: String
}

@Test func channelMetaConformsAndComposes() {
    #expect(PingPongMeta.name == "ping-pong")
    #expect(Ping.method == "Ping")
}
