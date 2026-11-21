import Foundation

/// 接続ライフサイクルの event。 reconnect は library ではなく caller の責務
/// (= TS と同方針)。 caller は `disconnected` を観測して再接続戦略を回す。
public enum ConnectionEvent: Sendable, Equatable {
    /// QUIC handshake 完了、 接続確立。
    case connected
    /// 接続切断。 `disconnect()` を呼んだときは reason が nil、 相手からの切断や
    /// 経路の失敗のときは、 判明していれば reason が付く。 1 接続につき 1 回だけ出る。
    case disconnected(reason: String?)
}

/// server が handshake で広告する自己記述 (= Rust `ServerIdentity` の対応物)。
public struct ServerIdentity: Sendable, Equatable {
    public let name: String
    public let version: String
    public let namespace: String
    /// server が公開する channel 名の一覧。
    public let channels: [String]

    public init(name: String, version: String, namespace: String, channels: [String]) {
        self.name = name
        self.version = version
        self.namespace = namespace
        self.channels = channels
    }
}

/// 確立済みの Unison 接続。 channel の open と lifecycle event を提供する。
public actor Connection {
    private let transport: any ChannelTransport
    private nonisolated let events: AsyncStream<ConnectionEvent>
    private nonisolated let eventSink: AsyncStream<ConnectionEvent>.Continuation
    private var isDisconnected = false
    private var identity: ServerIdentity?
    /// 進行中の identity の読み込み (接続で 1 本)。
    private var identityRead: Task<Void, Never>?
    /// 直前の読み込みが失敗した理由 (= 待ち始める前に終わっていた読み込みの error を返す)。
    private var identityFailure: (any Error)?
    private var identityWaiters: [(id: UInt64, cont: CheckedContinuation<ServerIdentity, any Error>)] = []
    private var nextIdentityWaiterID: UInt64 = 0

    init(transport: any ChannelTransport) {
        self.transport = transport
        let (stream, sink) = AsyncStream<ConnectionEvent>.makeStream()
        self.events = stream
        self.eventSink = sink
        sink.yield(.connected)
        // 相手からの切断 / 経路の失敗を transport から受け取る。
        let closed: @Sendable (String?) -> Void = { [weak self] reason in
            Task { [weak self] in await self?.transportClosed(reason: reason) }
        }
        Task { await transport.onClosed(closed) }
    }

    /// identity の読み込みは transport を掴んで待つので、 手放された接続では止める
    /// (= 止めないと transport が解放されず、 QUIC 接続が残る)。
    deinit {
        identityRead?.cancel()
    }

    /// 接続ライフサイクル event ストリーム。
    ///
    /// `.connected` のあと、 接続が終わると `.disconnected(reason:)` を 1 回流して
    /// 終端する。 `disconnect()` を呼んだ場合だけでなく、 server が接続を閉じた場合や
    /// 経路が失敗した場合にも出る。
    public nonisolated var connectionEvents: AsyncStream<ConnectionEvent> {
        events
    }

    /// server の自己記述を取得する (= identity stream を accept して読む)。
    ///
    /// server は identity を接続につき 1 回しか送らないので、 読み込みは接続で 1 本に
    /// まとめる。 同時に呼んでも全員が同じ値を受け取り、 一度読めたら以降は同じ値を返す。
    ///
    /// `timeout` (既定 5 秒) 以内に identity が届かなければ `UnisonError.timeout`。
    /// nil を渡すと期限を付けない。 期限や cancel で戻っても読み込みは続くので、
    /// あとから届けば次の呼び出しで読める。
    public func serverIdentity(
        timeout: Duration? = UnisonClient.defaultIdentityTimeout
    ) async throws -> ServerIdentity {
        if let identity { return identity }
        guard !isDisconnected else { throw UnisonError.notConnected }
        startIdentityReadIfNeeded()
        return try await withTimeout(timeout) { [weak self] in
            guard let self else { throw UnisonError.notConnected }
            return try await self.awaitIdentity()
        }
    }

    /// identity の読み込みが終わるのを待つ。 Task の cancel で `CancellationError`。
    private func awaitIdentity() async throws -> ServerIdentity {
        let id = nextIdentityWaiterID
        nextIdentityWaiterID += 1
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { cont in
                if let identity {
                    cont.resume(returning: identity)
                } else if Task.isCancelled {
                    cont.resume(throwing: CancellationError())
                } else if identityRead == nil {
                    // 待ち始める前に、 読み込みが失敗して終わっている。
                    cont.resume(throwing: identityFailure ?? UnisonError.notConnected)
                } else {
                    identityWaiters.append((id, cont))
                }
            }
        } onCancel: {
            Task { await self.cancelIdentityWaiter(id) }
        }
    }

    private func cancelIdentityWaiter(_ id: UInt64) {
        if let index = identityWaiters.firstIndex(where: { $0.id == id }) {
            identityWaiters.remove(at: index).cont.resume(throwing: CancellationError())
        }
    }

    private func startIdentityReadIfNeeded() {
        guard identity == nil, identityRead == nil else { return }
        identityFailure = nil
        let transport = self.transport
        identityRead = Task { [weak self] in
            let result: Result<ServerIdentity, any Error>
            do {
                guard let stream = try await transport.acceptStream() else {
                    throw UnisonError.notConnected
                }
                // 読み終えたら (失敗しても、 cancel されても) identity stream は閉じる。
                let identity = try await withTaskCancellationHandler {
                    do {
                        let identity = try await IdentityHandshake.read(from: stream)
                        await stream.close()
                        return identity
                    } catch {
                        await stream.close()
                        throw error
                    }
                } onCancel: {
                    Task { await stream.close() }
                }
                result = .success(identity)
            } catch {
                result = .failure(error)
            }
            await self?.identityReadFinished(result)
        }
    }

    private func identityReadFinished(_ result: Result<ServerIdentity, any Error>) {
        identityRead = nil
        switch result {
        case .success(let value):
            identity = value
        case .failure(let error):
            identityFailure = error
        }
        let waiters = identityWaiters
        identityWaiters.removeAll()
        for waiter in waiters {
            waiter.cont.resume(with: result)
        }
    }

    /// stream channel を開く (= 新 stream → `__channel:{name}` open → open_ack 待ち)。
    ///
    /// `timeout` (既定 5 秒) 以内に server が応えなければ `UnisonError.timeout`。
    /// nil を渡すと期限を付けない。 開けなかった場合 (拒否 / 期限 / cancel) は、
    /// 途中まで開いた stream を閉じる。
    public func openChannel<M: StreamChannelMeta>(
        _ meta: M,
        timeout: Duration? = UnisonClient.defaultOpenTimeout
    ) async throws -> StreamChannel<M> {
        _ = meta
        guard !isDisconnected else { throw UnisonError.notConnected }
        let transport = self.transport
        let name = M.name
        let core = try await withTimeout(timeout) {
            let stream = try await transport.openStream()
            let core = StreamChannelCore(name: name, stream: stream)
            await core.start()
            do {
                try await core.open()
            } catch {
                await core.close()
                throw error
            }
            return core
        }
        return StreamChannel(core: core)
    }

    /// credential を提示して connection を認証する (= connection-level authN、 v1.4.0)。
    ///
    /// `unison.auth` channel を open して `Authenticate` request を 1 回送り、 server の
    /// verifier 結果を待つ。`ok == false` (= 拒否) なら `UnisonError.authenticationDenied`
    /// を throw する。認証成功後は server 側がこの connection の principal を立てるので、
    /// 以降 open する channel は per-message gate を通過できる。**他 channel を open する前に
    /// 呼ぶこと**。
    ///
    /// server が `enable_auth` 未設定なら `unison.auth` が未登録で open が reject される。
    /// 設計: `design/connection-auth.md` §5.8。
    public func authenticate(_ credential: [UInt8]) async throws {
        let channel = try await openChannel(AuthChannelMeta())
        let result: AuthResult
        do {
            result = try await channel.request(AuthenticateRequest(credential: credential))
        } catch {
            await channel.close()
            throw error
        }
        await channel.close()
        guard result.ok else {
            throw UnisonError.authenticationDenied
        }
    }

    /// datagram channel を開く。
    public func openDatagramChannel<M: DatagramChannelMeta>(_ meta: M) async throws -> DatagramChannel<M> {
        _ = meta
        // TODO(next pass): datagram channel 登録 + channelId 紐づけ (QUIC datagram)。
        return DatagramChannel(name: M.name)
    }

    /// 接続を切断する。 開いている channel の stream も閉じる。 何度呼んでもよい。
    public func disconnect() async {
        finish(reason: nil)
        await transport.close()
    }

    /// transport が閉じた (= 相手からの切断 / 経路の失敗)。
    private func transportClosed(reason: String?) async {
        guard !isDisconnected else { return }
        finish(reason: reason)
        await transport.close()
    }

    private func finish(reason: String?) {
        guard !isDisconnected else { return }
        isDisconnected = true
        eventSink.yield(.disconnected(reason: reason))
        eventSink.finish()
    }
}
