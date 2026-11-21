import Foundation
import Network

/// `NWConnection`(QUIC stream)を [`ChannelStream`] に適合させる adapter。
///
/// `NWConnection` は内部で thread-safe (= 専用 queue でコールバック) なので
/// `@unchecked Sendable`。 send/receive の callback API を continuation で
/// async/await へ橋渡しする。
final class NWStreamChannel: ChannelStream, @unchecked Sendable {
    private let connection: NWConnection
    private let lock = NSLock()
    private var closedLocally = false

    /// 既に `.ready` まで起動済みの `NWConnection` を渡すこと。
    init(ready connection: NWConnection) {
        self.connection = connection
    }

    /// cancel されない `NWConnection` は QUIC 接続と UDP socket を生かし続けるので、
    /// 閉じ忘れても手放された時点で cancel する。
    deinit {
        connection.cancel()
    }

    /// 起動 + `.ready` 待ちをして wrap する。 `.ready` にならなかった場合
    /// (失敗 / Task の cancel) は connection を cancel してから throw する。
    static func start(_ connection: NWConnection, queue: DispatchQueue) async throws -> NWStreamChannel {
        let resumed = ResumeOnce()
        do {
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
                    connection.stateUpdateHandler = { state in
                        switch state {
                        case .ready:
                            if resumed.tryResume() { cont.resume() }
                        case .failed(let error):
                            if resumed.tryResume() { cont.resume(throwing: UnisonError.transport("stream failed: \(error)")) }
                        case .cancelled:
                            if resumed.tryResume() { cont.resume(throwing: UnisonError.notConnected) }
                        default:
                            break
                        }
                    }
                    connection.start(queue: queue)
                    // start より前に cancel されていた場合は、 onCancel の cancel() が
                    // start と前後しうるので、 ここでもう一度 cancel する (冪等)。
                    if Task.isCancelled { connection.cancel() }
                }
            } onCancel: {
                connection.cancel()
            }
        } catch {
            connection.cancel()
            throw Task.isCancelled ? CancellationError() : error
        }
        return NWStreamChannel(ready: connection)
    }

    func send(_ bytes: Data) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            connection.send(content: bytes, completion: .contentProcessed { error in
                if let error {
                    cont.resume(throwing: UnisonError.transport("stream send 失敗: \(error)"))
                } else {
                    cont.resume()
                }
            })
        }
    }

    func receive() async throws -> Data? {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data?, Error>) in
            connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [self] data, _, isComplete, error in
                if let error {
                    if isClosedLocally {
                        // 自分で閉じた stream の receive は error で終わる。 EOF として扱う。
                        cont.resume(returning: nil)
                    } else {
                        cont.resume(throwing: UnisonError.transport("stream receive 失敗: \(error)"))
                    }
                    return
                }
                if let data, !data.isEmpty {
                    cont.resume(returning: data)
                } else if isComplete {
                    cont.resume(returning: nil) // EOF
                } else {
                    cont.resume(returning: Data()) // 空 chunk、 caller が再度 receive
                }
            }
        }
    }

    func close() async {
        lock.withLock { closedLocally = true }
        connection.cancel()
    }

    private var isClosedLocally: Bool {
        lock.withLock { closedLocally }
    }
}

/// continuation の二重 resume を防ぐ最小 box。
final class ResumeOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func tryResume() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if done { return false }
        done = true
        return true
    }
}
