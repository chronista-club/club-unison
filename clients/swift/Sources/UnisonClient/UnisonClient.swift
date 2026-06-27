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

    /// endpoint へ接続し、 接続直後に credential を提示して認証してから [`Connection`] を
    /// 返す (= Rust `connect_with_credential` の対応物、 v1.4.0)。
    ///
    /// 認証が拒否された場合は connection を畳んで throw する。設計:
    /// `design/connection-auth.md` §5.8。
    public static func connect(
        to endpoint: Endpoint,
        trust: TrustPolicy,
        credential: [UInt8]
    ) async throws -> Connection {
        let conn = try await connect(to: endpoint, trust: trust)
        do {
            try await conn.authenticate(credential)
        } catch {
            await conn.disconnect()
            throw error
        }
        return conn
    }
}
