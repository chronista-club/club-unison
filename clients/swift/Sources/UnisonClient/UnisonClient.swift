import Foundation

/// Unison protocol の Swift client SDK エントリ。
///
/// ```swift
/// let conn = try await UnisonClient.connect(
///     to: .localDaemon(port: 7878),
///     trust: .skipVerify
/// )
/// let channel = try await conn.openChannel(SomeChannelMeta())
/// ```
///
/// transport = Apple `NWProtocolQUIC` (ALPN `"unison"`)、 wire = swift-protobuf。
/// reconnect は library ではなく caller の責務。
public enum UnisonClient {
    /// endpoint へ接続し、 QUIC handshake 完了後に [`Connection`] を返す。
    public static func connect(to endpoint: Endpoint, trust: TrustPolicy) async throws -> Connection {
        let transport = try await QUICTransport.connect(to: endpoint, trust: trust)
        return Connection(transport: transport)
    }
}
