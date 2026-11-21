import Foundation

/// stream channel の状態機械 (= TS `unison_channel.ts` の Swift port)。
///
/// 1 本の [`ChannelStream`] 上で:
/// - **open**: `__channel:{name}` request → server の `__channel_ack` を await
///   (response = accept / error = nack)。
/// - **request/response**: id 採番して送信、 同 id の response/error を await。
/// - **event**: server push (event/request msgType) を溜め、 `nextEvent()` で渡す。
///
/// 受信は単一 recv loop。 `msgType` で pending request の resolve と event push を
/// 振り分ける (= Rust `network/channel.rs` と同じ dispatch)。
///
/// 待ち (open / request / nextEvent) は必ず戻る: 応答、 Task の cancel、 受信の終わり
/// (EOF / 壊れた frame / `close()`) のどれかで resume する。 受信が終わったあとの
/// 送信は待たずに失敗する。
actor StreamChannelCore {
    /// 読まれていない event を溜める上限 (= Rust `UnisonChannel` の受信 queue と同じ)。
    /// 満杯のあいだに届いた event は捨てる。
    static let eventBufferLimit = 256

    let name: String
    private let stream: any ChannelStream

    private var nextID: UInt64 = 1
    private var pending: [UInt64: CheckedContinuation<Protocol_ProtocolMessage, Error>] = [:]
    private var recvTask: Task<Void, Never>?
    /// 受信が終わった理由。 入っていれば、 以降の送信は待たずにこの error で失敗する。
    private var terminal: (any Error)?

    private var events: [Protocol_ProtocolMessage] = []
    private var eventWaiters: [(id: UInt64, cont: CheckedContinuation<Protocol_ProtocolMessage?, Never>)] = []
    private var nextWaiterID: UInt64 = 0

    init(name: String, stream: any ChannelStream) {
        self.name = name
        self.stream = stream
    }

    /// `close()` を呼ばずに手放された channel の stream を閉じる
    /// (= recv loop は core を弱参照で持つので、 handle が消えればここに来る)。
    deinit {
        recvTask?.cancel()
        if terminal == nil {
            let stream = self.stream
            Task { await stream.close() }
        }
    }

    /// recv loop を起動する (= open / request の前に呼ぶ)。
    func start() {
        guard recvTask == nil else { return }
        let stream = self.stream
        recvTask = Task { [weak self] in
            var reader = FrameReader()
            // 何も throw されずに loop を抜けたら EOF (stream 終端)。
            var failure: any Error = UnisonError.notConnected
            do {
                while let chunk = try await stream.receive() {
                    reader.append(chunk)
                    var messages: [Protocol_ProtocolMessage] = []
                    var broken: (any Error)?
                    do {
                        while let body = try reader.nextFrame() {
                            if case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) {
                                messages.append(msg)
                            }
                        }
                    } catch {
                        broken = error
                    }
                    // 待っている間は core を掴まない (= handle が消えたら loop も終わる)。
                    guard let core = self else { return }
                    await core.dispatch(messages)
                    if let broken { throw broken }
                }
            } catch {
                failure = error
            }
            await self?.terminate(failure)
        }
    }

    /// `__channel:{name}` open handshake。 ack が error なら throw。
    func open() async throws {
        let id = allocID()
        var msg = Protocol_ProtocolMessage()
        msg.id = id
        msg.method = "__channel:" + name
        msg.msgType = .request
        let ack = try await sendAndAwait(id: id, message: msg)
        switch ack.msgType {
        case .response:
            return
        case .error:
            let reason = String(data: ack.payload, encoding: .utf8) ?? "open rejected"
            throw UnisonError.channelRejected(channel: name, reason: reason)
        default:
            throw UnisonError.channelRejected(channel: name, reason: "unexpected ack msgType")
        }
    }

    /// request を 1 本送り、 対応する response payload を返す。
    func request(method: String, payload: Data) async throws -> Data {
        let id = allocID()
        var msg = Protocol_ProtocolMessage()
        msg.id = id
        msg.method = method
        msg.msgType = .request
        msg.payload = payload
        let resp = try await sendAndAwait(id: id, message: msg)
        if resp.msgType == .error {
            throw UnisonError.transport(String(data: resp.payload, encoding: .utf8) ?? "request error")
        }
        return resp.payload
    }

    /// 次の server push event を返す。 channel が終わっていて、 溜まった event も
    /// 無ければ nil。 Task の cancel でも nil で戻る。
    ///
    /// 読み手は 1 つを想定する。 同時に複数から呼ぶと、 event は読み手に振り分けられる
    /// (複製はしない)。
    func nextEvent() async -> Protocol_ProtocolMessage? {
        let id = nextWaiterID
        nextWaiterID += 1
        return await withTaskCancellationHandler {
            await withCheckedContinuation { cont in
                if !events.isEmpty {
                    cont.resume(returning: events.removeFirst())
                } else if terminal != nil || Task.isCancelled {
                    cont.resume(returning: nil)
                } else {
                    eventWaiters.append((id, cont))
                }
            }
        } onCancel: {
            Task { await self.cancelEventWaiter(id) }
        }
    }

    /// channel を閉じる (= pending を全 reject、 溜まった event を捨て、 stream close)。
    func close() async {
        events.removeAll()
        await terminate(UnisonError.notConnected)
    }

    // MARK: - internal

    private func allocID() -> UInt64 {
        defer { nextID += 1 }
        return nextID
    }

    /// pending を登録してから frame を送り、 id 対応の応答を待つ。
    /// (ack が先着しても取りこぼさないよう、 send 前に pending 登録。)
    private func sendAndAwait(
        id: UInt64,
        message: Protocol_ProtocolMessage
    ) async throws -> Protocol_ProtocolMessage {
        let frame = try Framing.encodeProtocolFrame(message)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { cont in
                if let terminal {
                    // 受信が終わっているので、 応答は二度と来ない。
                    cont.resume(throwing: terminal)
                    return
                }
                if Task.isCancelled {
                    cont.resume(throwing: CancellationError())
                    return
                }
                pending[id] = cont
                Task {
                    do {
                        try await stream.send(frame)
                    } catch {
                        resumePending(id: id, throwing: error)
                    }
                }
            }
        } onCancel: {
            Task { await self.resumePending(id: id, throwing: CancellationError()) }
        }
    }

    private func resumePending(id: UInt64, throwing error: Error) {
        if let cont = pending.removeValue(forKey: id) {
            cont.resume(throwing: error)
        }
    }

    private func cancelEventWaiter(_ id: UInt64) {
        if let index = eventWaiters.firstIndex(where: { $0.id == id }) {
            eventWaiters.remove(at: index).cont.resume(returning: nil)
        }
    }

    private func dispatch(_ messages: [Protocol_ProtocolMessage]) {
        for msg in messages {
            switch msg.msgType {
            case .response, .error:
                // id 相関で pending を resolve (= open_ack / request response)。
                // cancel / timeout 済みの id や未知の id は捨てる。
                if let cont = pending.removeValue(forKey: msg.id) {
                    cont.resume(returning: msg)
                }
            case .event, .request:
                deliverEvent(msg)
            default:
                break
            }
        }
    }

    private func deliverEvent(_ msg: Protocol_ProtocolMessage) {
        guard terminal == nil else { return }
        if !eventWaiters.isEmpty {
            eventWaiters.removeFirst().cont.resume(returning: msg)
        } else if events.count < Self.eventBufferLimit {
            events.append(msg)
        }
        // 満杯なら捨てる (= Rust の `try_send` 失敗と同じ。 読み手が追いつくまで新しい方を落とす)。
    }

    /// 受信の終わり。 待っている request / event の読み手を全部起こし、 stream を閉じる。
    /// 溜まっている event は、 読み手が読み切れるよう残す。
    private func terminate(_ error: any Error) async {
        guard terminal == nil else { return }
        terminal = error
        recvTask?.cancel()

        let conts = pending.values
        pending.removeAll()
        for cont in conts {
            cont.resume(throwing: error)
        }
        let waiters = eventWaiters
        eventWaiters.removeAll()
        for waiter in waiters {
            waiter.cont.resume(returning: nil)
        }

        await stream.close()
    }
}
