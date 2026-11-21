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

    /// この接続で開いた / 受けた stream。 cancel されない stream (`NWConnection`) が
    /// 1 本でも残ると QUIC 接続と UDP socket が残るので、 `close()` で全部閉じる。
    private var streams: [WeakStream] = []
    private var isClosed = false
    private var didNotifyClosed = false
    private var closedReason: String?
    private var closeHandler: (@Sendable (String?) -> Void)?

    private struct WeakStream {
        weak var stream: NWStreamChannel?
    }

    private init(multiplex: NWMultiplexGroup, group: NWConnectionGroup) {
        self.multiplex = multiplex
        self.group = group
    }

    /// `close()` を呼ばずに手放された接続を畳む (= cancel されない group は QUIC 接続と
    /// UDP socket を生かし続ける)。 stream は `NWStreamChannel` の deinit が cancel する。
    deinit {
        group.cancel()
    }

    /// QUIC 接続を確立し、 group が `.ready` になるまで待つ。
    static func connect(to endpoint: Endpoint, trust: TrustPolicy) async throws -> QUICTransport {
        let nwEndpoint = try resolve(endpoint)

        let quic = NWProtocolQUIC.Options(alpn: [alpn])
        applyTrust(trust, to: quic.securityProtocolOptions)
        let params = NWParameters(quic: quic)
        // Opt in to QUIC connection migration (e.g. Wi-Fi → cellular).
        // This preserves the session when possible; it does not reconnect it.
        params.multipathServiceType = .handover

        let multiplex = NWMultiplexGroup(to: nwEndpoint)
        let group = NWConnectionGroup(with: multiplex, using: params)
        let transport = QUICTransport(multiplex: multiplex, group: group)
        try await transport.startGroup()
        return transport
    }

    /// group を起動して `.ready` を待つ。 `.ready` にならなかった場合 (失敗 / 10 秒の
    /// 期限 = `UnisonError.timeout` / Task の cancel) は group を cancel してから throw する。
    ///
    /// `.ready` のあとに group が `.failed` / `.cancelled` になったら、 接続が閉じたと
    /// みなして `onClosed` の handler に知らせる。
    private func startGroup() async throws {
        let resumed = ResumeOnce()
        let group = self.group
        let callbackQueue = self.callbackQueue
        do {
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                    group.stateUpdateHandler = { [weak self] state in
                        switch state {
                        case .ready:
                            if resumed.tryResume() { cont.resume() }
                        case .failed(let error):
                            if resumed.tryResume() {
                                cont.resume(throwing: UnisonError.transport("group failed: \(error)"))
                            } else {
                                Task { await self?.groupClosed(reason: "connection failed: \(error)") }
                            }
                        case .cancelled:
                            if resumed.tryResume() {
                                cont.resume(throwing: UnisonError.notConnected)
                            } else {
                                Task { await self?.groupClosed(reason: nil) }
                            }
                        default:
                            // `.waiting` は ready 後にも (loopback で ENETDOWN として) 出るが
                            // stream は正常に開けるため無視する。 真の接続不能は下の timeout で拾う。
                            break
                        }
                    }
                    // server 起点 stream (= identity) を受ける。
                    group.newConnectionHandler = { [weak self] conn in
                        guard let self else {
                            conn.cancel()
                            return
                        }
                        Task { await self.handleIncoming(conn) }
                    }
                    group.start(queue: callbackQueue)
                    // ready が来ない (= server 不在等) 場合の脱出。
                    callbackQueue.asyncAfter(deadline: .now() + 10) {
                        if resumed.tryResume() {
                            cont.resume(throwing: UnisonError.timeout)
                        }
                    }
                }
            } onCancel: {
                group.cancel()
            }
        } catch {
            group.cancel()
            throw Task.isCancelled ? CancellationError() : error
        }
    }

    private func handleIncoming(_ conn: NWConnection) async {
        guard !isClosed, let stream = try? await NWStreamChannel.start(conn, queue: callbackQueue) else {
            conn.cancel()
            return
        }
        // start を待つ間に閉じられていたら、 誰にも渡さずに閉じる。
        guard !isClosed else {
            await stream.close()
            return
        }
        track(stream)
        await accepted.push(stream)
    }

    private func track(_ stream: NWStreamChannel) {
        streams.removeAll { $0.stream == nil }
        streams.append(WeakStream(stream: stream))
    }

    /// stream と group を片付ける (冪等)。
    private func shutdown() async {
        guard !isClosed else { return }
        isClosed = true
        let live = streams.compactMap(\.stream)
        streams.removeAll()
        for stream in live {
            await stream.close()
        }
        group.cancel()
        await accepted.finish()
    }

    /// ready のあとに group が終わった (= 相手からの切断 / 経路の失敗 / 自分の cancel)。
    ///
    /// 理由は片付けを待つ前に記録する (= `.failed` のあとに続く `.cancelled` が、
    /// 片付けの待ちのあいだに理由なしで先に通知するのを防ぐ)。
    private func groupClosed(reason: String?) async {
        notifyClosed(reason: reason)
        await shutdown()
    }

    private func notifyClosed(reason: String?) {
        guard !didNotifyClosed else { return }
        didNotifyClosed = true
        closedReason = reason
        closeHandler?(reason)
        closeHandler = nil
    }

    // MARK: - ChannelTransport 適合

    func openStream() async throws -> any ChannelStream {
        guard !isClosed else { throw UnisonError.notConnected }
        guard let conn = NWConnection(from: group) else {
            throw UnisonError.transport("openStream: NWConnection(from: group) が nil")
        }
        let stream = try await NWStreamChannel.start(conn, queue: callbackQueue)
        // start を待つ間に閉じられていたら、 渡さずに閉じる。
        guard !isClosed else {
            await stream.close()
            throw UnisonError.notConnected
        }
        track(stream)
        return stream
    }

    func acceptStream() async throws -> (any ChannelStream)? {
        await accepted.pop()
    }

    func close() async {
        await shutdown()
        notifyClosed(reason: nil)
    }

    func onClosed(_ handler: @escaping @Sendable (String?) -> Void) async {
        if didNotifyClosed {
            handler(closedReason)
        } else {
            closeHandler = handler
        }
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

/// accept 待ち server-起点 stream の queue。
///
/// - 待ち (`pop`) は、 stream が届く / `finish` / Task の cancel のどれかで戻る。
///   待ちが複数あれば、 届いた順に 1 本ずつ渡す。
/// - 受け取り手のいない stream は `bufferLimit` 本まで溜める。 それを超えた分と、
///   `finish` の時点で残っていた分は閉じる (= 閉じない stream は接続を生かし続ける)。
actor PendingStreams {
    static let bufferLimit = 16

    private var buffer: [any ChannelStream] = []
    private var finished = false
    private var waiters: [(id: UInt64, cont: CheckedContinuation<(any ChannelStream)?, Never>)] = []
    private var nextWaiterID: UInt64 = 0

    func push(_ stream: any ChannelStream) async {
        if finished {
            await stream.close()
        } else if !waiters.isEmpty {
            waiters.removeFirst().cont.resume(returning: stream)
        } else if buffer.count < Self.bufferLimit {
            buffer.append(stream)
        } else {
            await stream.close()
        }
    }

    func finish() async {
        finished = true
        let waiting = waiters
        waiters.removeAll()
        for waiter in waiting {
            waiter.cont.resume(returning: nil)
        }
        let leftover = buffer
        buffer.removeAll()
        for stream in leftover {
            await stream.close()
        }
    }

    func pop() async -> (any ChannelStream)? {
        let id = nextWaiterID
        nextWaiterID += 1
        return await withTaskCancellationHandler {
            await withCheckedContinuation { cont in
                if !buffer.isEmpty {
                    cont.resume(returning: buffer.removeFirst())
                } else if finished || Task.isCancelled {
                    cont.resume(returning: nil)
                } else {
                    waiters.append((id, cont))
                }
            }
        } onCancel: {
            Task { await self.cancelWaiter(id) }
        }
    }

    private func cancelWaiter(_ id: UInt64) {
        if let index = waiters.firstIndex(where: { $0.id == id }) {
            waiters.remove(at: index).cont.resume(returning: nil)
        }
    }
}
