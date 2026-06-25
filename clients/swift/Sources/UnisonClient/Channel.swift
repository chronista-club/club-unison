import Foundation

/// stream channel ハンドル (= request/response + server push event)。
///
/// `Connection.openChannel(_:)` が返す。 `M` で event / request の型が静的に決まる。
public struct StreamChannel<M: StreamChannelMeta>: Sendable {
    private let core: StreamChannelCore

    init(core: StreamChannelCore) {
        self.core = core
    }

    /// server → client の push event ストリーム。 wire payload を `M.Event` へ
    /// JSON decode する。 decode 不能な event は skip する。
    public var events: AsyncStream<M.Event> {
        let raw = core.eventStream
        return AsyncStream { continuation in
            let task = Task {
                let decoder = JSONDecoder()
                for await msg in raw {
                    if let event = try? decoder.decode(M.Event.self, from: msg.payload) {
                        continuation.yield(event)
                    }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// request を 1 本送り、 対応する response を待つ。
    public func request<R: UnisonRequest>(_ request: R) async throws -> R.Response {
        let payload: Data
        do {
            payload = try JSONEncoder().encode(request)
        } catch {
            throw UnisonError.codec("request encode 失敗: \(error)")
        }
        let responseBytes = try await core.request(method: R.method, payload: payload)
        do {
            return try JSONDecoder().decode(R.Response.self, from: responseBytes)
        } catch {
            throw UnisonError.codec("response decode 失敗: \(error)")
        }
    }

    /// channel を閉じる。
    public func close() async {
        await core.close()
    }
}

/// datagram channel ハンドル (= unreliable な server push event 専用)。
///
/// QUIC datagram (RFC9221) の demux は後続 pass。 現状は型 surface のみ。
public struct DatagramChannel<M: DatagramChannelMeta>: Sendable {
    private let name: String

    init(name: String) {
        self.name = name
    }

    /// server → client の push event ストリーム (QUIC datagram)。
    public var events: AsyncStream<M.Event> {
        // TODO(next pass): datagram demux → M.Event decode。
        AsyncStream { $0.finish() }
    }

    /// channel を閉じる。
    public func close() async {
        // TODO(next pass): datagram subscription tear down。
    }
}
