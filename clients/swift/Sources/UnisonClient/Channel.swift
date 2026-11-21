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
    ///
    /// - 読むのを途中でやめても (`break`) channel は開いたままで、 もう一度 `events` を
    ///   回せば続きから読める。
    /// - 相手が channel を終えた場合 (server が stream を閉じた / 接続が切れた) は、
    ///   溜まった分を読み切ってから終端する。 自分で `close()` した場合は、 溜まった分を
    ///   捨てて終端する。
    /// - 読まれていない event は 256 件まで溜める。 それを超えて届いた分は捨てる
    ///   (= Rust `UnisonChannel` の受信 queue と同じ)。
    /// - 読み手は 1 つを想定する。 同時に複数の loop で読むと、 event は複製されずに
    ///   振り分けられる。
    public var events: AsyncStream<M.Event> {
        let core = self.core
        return AsyncStream(unfolding: {
            let decoder = JSONDecoder()
            while let msg = await core.nextEvent() {
                if let event = try? decoder.decode(M.Event.self, from: msg.payload) {
                    return event
                }
            }
            return nil
        })
    }

    /// request を 1 本送り、 対応する response を待つ。
    ///
    /// `timeout` (既定 30 秒 = Rust `UnisonChannel` と同じ) 以内に応答が無ければ
    /// `UnisonError.timeout`。 nil を渡すと期限を付けない。 Task が cancel されたら
    /// `CancellationError` で戻る。 どちらの場合も、 遅れて届いた応答は捨てる。
    public func request<R: UnisonRequest>(
        _ request: R,
        timeout: Duration? = UnisonClient.defaultRequestTimeout
    ) async throws -> R.Response {
        let payload: Data
        do {
            payload = try JSONEncoder().encode(request)
        } catch {
            throw UnisonError.codec("request encode 失敗: \(error)")
        }
        let core = self.core
        let method = R.method
        let responseBytes = try await withTimeout(timeout) {
            try await core.request(method: method, payload: payload)
        }
        do {
            return try JSONDecoder().decode(R.Response.self, from: responseBytes)
        } catch {
            throw UnisonError.codec("response decode 失敗: \(error)")
        }
    }

    /// channel を閉じる。 待っている request は `UnisonError.notConnected` で戻る。
    ///
    /// 閉じ忘れても、 handle を手放せば stream は閉じる。
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
