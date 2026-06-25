import Foundation

/// stream channel の状態機械 (= TS `unison_channel.ts` の Swift port)。
///
/// 1 本の [`ChannelStream`] 上で:
/// - **open**: `__channel:{name}` request → server の `__channel_ack` を await
///   (response = accept / error = nack)。
/// - **request/response**: id 採番して送信、 同 id の response/error を await。
/// - **event**: server push (event/request msgType) を `eventStream` へ流す。
///
/// 受信は単一 recv loop。 `msgType` で pending request の resolve と event push を
/// 振り分ける (= Rust `network/channel.rs` と同じ dispatch)。
actor StreamChannelCore {
    let name: String
    private let stream: any ChannelStream

    private var nextID: UInt64 = 1
    private var pending: [UInt64: CheckedContinuation<Protocol_ProtocolMessage, Error>] = [:]
    private var recvTask: Task<Void, Never>?
    private var closed = false

    /// server push event の生メッセージ列 (= 公開層が M.Event へ decode する)。
    nonisolated let eventStream: AsyncStream<Protocol_ProtocolMessage>
    private nonisolated let eventSink: AsyncStream<Protocol_ProtocolMessage>.Continuation

    init(name: String, stream: any ChannelStream) {
        self.name = name
        self.stream = stream
        let (s, c) = AsyncStream<Protocol_ProtocolMessage>.makeStream()
        self.eventStream = s
        self.eventSink = c
    }

    /// recv loop を起動する (= open / request の前に呼ぶ)。
    func start() {
        guard recvTask == nil else { return }
        recvTask = Task { await self.recvLoop() }
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

    /// channel を閉じる (= pending を全 reject、 recv loop 停止、 stream close)。
    func close() async {
        guard !closed else { return }
        closed = true
        recvTask?.cancel()
        failAll(UnisonError.notConnected)
        await stream.close()
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
        return try await withCheckedThrowingContinuation { cont in
            pending[id] = cont
            Task {
                do {
                    try await stream.send(frame)
                } catch {
                    resumePending(id: id, throwing: error)
                }
            }
        }
    }

    private func resumePending(id: UInt64, throwing error: Error) {
        if let cont = pending.removeValue(forKey: id) {
            cont.resume(throwing: error)
        }
    }

    private func recvLoop() async {
        var reader = FrameReader()
        do {
            while let chunk = try await stream.receive() {
                reader.append(chunk)
                while let body = try reader.nextFrame() {
                    if case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) {
                        dispatch(msg)
                    }
                }
            }
        } catch {
            failAll(error)
            return
        }
        // EOF (stream 終端)。
        failAll(UnisonError.notConnected)
    }

    private func dispatch(_ msg: Protocol_ProtocolMessage) {
        switch msg.msgType {
        case .response, .error:
            // id 相関で pending を resolve (= open_ack / request response)。
            if let cont = pending.removeValue(forKey: msg.id) {
                cont.resume(returning: msg)
            }
        case .event, .request:
            eventSink.yield(msg)
        default:
            break
        }
    }

    private func failAll(_ error: Error) {
        let conts = pending.values
        pending.removeAll()
        for cont in conts {
            cont.resume(throwing: error)
        }
        eventSink.finish()
    }
}
