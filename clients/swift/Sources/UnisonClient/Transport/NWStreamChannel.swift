import Foundation
import Network

/// `NWConnection`(QUIC stream)を [`ChannelStream`] に適合させる adapter。
///
/// `NWConnection` は内部で thread-safe (= 専用 queue でコールバック) なので
/// `@unchecked Sendable`。 send/receive の callback API を continuation で
/// async/await へ橋渡しする。
final class NWStreamChannel: ChannelStream, @unchecked Sendable {
    private let connection: NWConnection

    /// 既に `.ready` まで起動済みの `NWConnection` を渡すこと。
    init(ready connection: NWConnection) {
        self.connection = connection
    }

    /// 起動 + `.ready` 待ちをして wrap する。
    static func start(_ connection: NWConnection, queue: DispatchQueue) async throws -> NWStreamChannel {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let resumed = ResumeOnce()
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
            connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { data, _, isComplete, error in
                if let error {
                    cont.resume(throwing: UnisonError.transport("stream receive 失敗: \(error)"))
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
        connection.cancel()
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
