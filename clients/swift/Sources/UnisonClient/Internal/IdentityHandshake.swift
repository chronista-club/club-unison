import Foundation

/// Identity handshake — server 自己紹介の受信 (= TS `identity.ts` の Swift port)。
///
/// Unison server は接続直後に bidi stream を 1 本 open し、 そこへ `__identity`
/// の `ProtocolMessage` (= msgType event、 payload は JSON 化した ServerIdentity)
/// を 1 本送って stream を finish する (= Rust `quic.rs::handle_connection`)。
enum IdentityHandshake {
    static let method = "__identity"

    /// 1 本の identity stream を drain して [`ServerIdentity`] を返す。
    static func read(from stream: any ChannelStream) async throws -> ServerIdentity {
        var reader = FrameReader()
        while let chunk = try await stream.receive() {
            reader.append(chunk)
            while let body = try reader.nextFrame() {
                guard case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) else {
                    throw UnisonError.codec("identity: PROTOCOL frame を期待")
                }
                guard msg.method == method else {
                    throw UnisonError.codec("identity: method \"\(method)\" を期待、 got \"\(msg.method)\"")
                }
                return try parse(msg.payload)
            }
        }
        throw UnisonError.notConnected
    }

    /// JSON payload を [`ServerIdentity`] へ。 channels は名前だけ抽出する。
    static func parse(_ payload: Data) throws -> ServerIdentity {
        struct WireIdentity: Decodable {
            let name: String
            let version: String
            let namespace: String
            struct WireChannel: Decodable { let name: String }
            let channels: [WireChannel]
        }
        do {
            let wire = try JSONDecoder().decode(WireIdentity.self, from: payload)
            return ServerIdentity(
                name: wire.name,
                version: wire.version,
                namespace: wire.namespace,
                channels: wire.channels.map(\.name)
            )
        } catch {
            throw UnisonError.codec("identity: ServerIdentity JSON parse 失敗: \(error)")
        }
    }
}
