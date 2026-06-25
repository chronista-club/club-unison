import Foundation
import Network
import Security

/// Apple `Network.framework` の `NWProtocolQUIC` を使う raw QUIC transport。
///
/// `NWMultiplexGroup` + `NWConnectionGroup` で 1 本の QUIC 接続に複数 stream を
/// 多重化する。 client 起点 stream は `NWConnection(from: group)`、 server 起点
/// stream (= identity) は `newConnectionHandler` 経由で受ける。
///
/// ALPN は `"unison"` 固定 (= Rust `network::UNISON_ALPN` と一致。 QUIC は
/// RFC 9001 §8.1 で ALPN 必須)。
actor QUICTransport: ChannelTransport {
    /// raw QUIC 経路の ALPN。 Rust `network::UNISON_ALPN` と一致させること。
    static let alpn = "unison"

    private let multiplex: NWMultiplexGroup
    private let group: NWConnectionGroup
    private let callbackQueue = DispatchQueue(label: "club.chronista.unison.quic")
    private let accepted = PendingStreams()

    private init(multiplex: NWMultiplexGroup, group: NWConnectionGroup) {
        self.multiplex = multiplex
        self.group = group
    }

    /// QUIC 接続を確立し、 group が `.ready` になるまで待つ。
    static func connect(to endpoint: Endpoint, trust: TrustPolicy) async throws -> QUICTransport {
        let nwEndpoint = try resolve(endpoint)

        let quic = NWProtocolQUIC.Options(alpn: [alpn])
        applyTrust(trust, to: quic.securityProtocolOptions)
        let params = NWParameters(quic: quic)

        let multiplex = NWMultiplexGroup(to: nwEndpoint)
        let group = NWConnectionGroup(with: multiplex, using: params)
        let transport = QUICTransport(multiplex: multiplex, group: group)
        try await transport.startGroup()
        return transport
    }

    private func startGroup() async throws {
        let resumed = ResumeOnce()
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            group.stateUpdateHandler = { state in
                switch state {
                case .ready:
                    if resumed.tryResume() { cont.resume() }
                case .failed(let error):
                    if resumed.tryResume() { cont.resume(throwing: UnisonError.transport("group failed: \(error)")) }
                case .cancelled:
                    if resumed.tryResume() { cont.resume(throwing: UnisonError.notConnected) }
                default:
                    // `.waiting` は ready 後にも (loopback で ENETDOWN として) 出るが
                    // stream は正常に開けるため無視する。 真の接続不能は下の timeout で拾う。
                    break
                }
            }
            // server 起点 stream (= identity) を受ける。
            group.newConnectionHandler = { [weak self] conn in
                guard let self else { return }
                Task { await self.handleIncoming(conn) }
            }
            group.start(queue: callbackQueue)
            // ready が来ない (= server 不在等) 場合の脱出。
            callbackQueue.asyncAfter(deadline: .now() + 10) {
                if resumed.tryResume() {
                    cont.resume(throwing: UnisonError.transport("QUIC connect timeout"))
                }
            }
        }
    }

    private func handleIncoming(_ conn: NWConnection) async {
        if let stream = try? await NWStreamChannel.start(conn, queue: callbackQueue) {
            await accepted.push(stream)
        }
    }

    // MARK: - ChannelTransport 適合

    func openStream() async throws -> any ChannelStream {
        guard let conn = NWConnection(from: group) else {
            throw UnisonError.transport("openStream: NWConnection(from: group) が nil")
        }
        return try await NWStreamChannel.start(conn, queue: callbackQueue)
    }

    func acceptStream() async throws -> (any ChannelStream)? {
        await accepted.pop()
    }

    func close() async {
        group.cancel()
        await accepted.finish()
    }

    // MARK: - Endpoint / trust 解決

    private static func resolve(_ endpoint: Endpoint) throws -> NWEndpoint {
        switch endpoint {
        case .localDaemon(let port):
            return .hostPort(host: "::1", port: nwPort(port))
        case .host(let host, let port):
            return .hostPort(host: NWEndpoint.Host(host), port: nwPort(port))
        case .bonjour:
            throw UnisonError.notImplemented("Endpoint.bonjour")
        }
    }

    private static func nwPort(_ raw: UInt16) -> NWEndpoint.Port {
        NWEndpoint.Port(rawValue: raw)!
    }

    private static func applyTrust(_ trust: TrustPolicy, to options: sec_protocol_options_t) {
        switch trust {
        case .system:
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

/// accept 待ち server-起点 stream の continuation ベース queue。
actor PendingStreams {
    private var buffer: [any ChannelStream] = []
    private var finished = false
    private var waiter: CheckedContinuation<(any ChannelStream)?, Never>?

    func push(_ stream: any ChannelStream) {
        if let w = waiter {
            waiter = nil
            w.resume(returning: stream)
        } else {
            buffer.append(stream)
        }
    }

    func finish() {
        finished = true
        if let w = waiter {
            waiter = nil
            w.resume(returning: nil)
        }
    }

    func pop() async -> (any ChannelStream)? {
        if !buffer.isEmpty { return buffer.removeFirst() }
        if finished { return nil }
        return await withCheckedContinuation { cont in waiter = cont }
    }
}
