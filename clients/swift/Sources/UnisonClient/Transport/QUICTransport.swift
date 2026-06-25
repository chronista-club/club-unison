import Foundation
import Network
import Security

/// Apple `Network.framework` の `NWProtocolQUIC` を使う raw QUIC transport。
///
/// ALPN は `"unison"` 固定 (= Rust 側 `network::UNISON_ALPN` と一致。 QUIC は
/// RFC 9001 §8.1 で ALPN 必須)。 framing / channel mux はこの上の層が担う。
actor QUICTransport: ChannelTransport {
    /// raw QUIC 経路の ALPN。 Rust `network::UNISON_ALPN` と一致させること。
    static let alpn = "unison"

    private let connection: NWConnection

    private init(connection: NWConnection) {
        self.connection = connection
    }

    /// QUIC 接続を確立し、 `.ready` (handshake 完了) まで待つ。
    static func connect(to endpoint: Endpoint, trust: TrustPolicy) async throws -> QUICTransport {
        let nwEndpoint = try Self.resolve(endpoint)

        let quic = NWProtocolQUIC.Options(alpn: [Self.alpn])
        Self.applyTrust(trust, to: quic.securityProtocolOptions)

        let params = NWParameters(quic: quic)
        let connection = NWConnection(to: nwEndpoint, using: params)
        let transport = QUICTransport(connection: connection)
        try await transport.start()
        return transport
    }

    private func start() async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            // continuation を 1 回だけ resume する guard。
            let resumed = LockedBox(false)
            connection.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if resumed.swap(true) == false { cont.resume() }
                case .failed(let error):
                    if resumed.swap(true) == false {
                        cont.resume(throwing: UnisonError.transport("\(error)"))
                    }
                case .waiting(let error):
                    // server 不在等。 ここで諦める (auto-reconnect は caller 責務)。
                    if resumed.swap(true) == false {
                        cont.resume(throwing: UnisonError.transport("waiting: \(error)"))
                    }
                default:
                    break
                }
            }
            connection.start(queue: .global())
        }
    }

    /// 接続を閉じる。
    func cancel() {
        connection.cancel()
    }

    // MARK: - ChannelTransport 適合

    /// client 起点の bidi QUIC stream を開く。
    /// TODO(next pass): NWMultiplexGroup / NWConnectionGroup で stream を払い出す。
    func openStream() async throws -> any ChannelStream {
        throw UnisonError.notImplemented("QUICTransport.openStream (NWProtocolQUIC stream は次 pass)")
    }

    /// server 起点の bidi QUIC stream を受け入れる (= identity stream)。
    /// TODO(next pass): group の newConnectionHandler から払い出す。
    func acceptStream() async throws -> (any ChannelStream)? {
        throw UnisonError.notImplemented("QUICTransport.acceptStream (NWProtocolQUIC stream は次 pass)")
    }

    func close() async {
        connection.cancel()
    }

    // MARK: - Endpoint / trust 解決

    private static func resolve(_ endpoint: Endpoint) throws -> NWEndpoint {
        switch endpoint {
        case .localDaemon(let port):
            return .hostPort(host: "::1", port: Self.port(port))
        case .host(let host, let port):
            return .hostPort(host: NWEndpoint.Host(host), port: Self.port(port))
        case .bonjour:
            // TODO(next pass): NWEndpoint.service / NWBrowser による discovery。
            throw UnisonError.notImplemented("Endpoint.bonjour")
        }
    }

    private static func port(_ raw: UInt16) -> NWEndpoint.Port {
        NWEndpoint.Port(rawValue: raw)!
    }

    private static func applyTrust(_ trust: TrustPolicy, to options: sec_protocol_options_t) {
        switch trust {
        case .system:
            // default 検証 (verify block を設定しない)。
            break
        case .skipVerify:
            sec_protocol_options_set_verify_block(
                options,
                { _, _, complete in complete(true) },
                DispatchQueue.global()
            )
        case .pinned(let der):
            sec_protocol_options_set_verify_block(
                options,
                { _, trustRef, complete in
                    let trust = sec_trust_copy_ref(trustRef).takeRetainedValue()
                    guard let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
                          let leaf = chain.first
                    else {
                        complete(false)
                        return
                    }
                    let leafDer = SecCertificateCopyData(leaf) as Data
                    complete(leafDer == der)
                },
                DispatchQueue.global()
            )
        }
    }
}

/// continuation の二重 resume を防ぐ最小の同期 box。
private final class LockedBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Bool
    init(_ value: Bool) { self.value = value }
    /// 旧値を返しつつ新値を入れる。
    func swap(_ new: Bool) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let old = value
        value = new
        return old
    }
}
