import Foundation

/// stream channel ハンドル (= request/response + server push event)。
///
/// `Connection.openChannel(_:)` が返す。 `M` で event / request の型が静的に決まる。
public struct StreamChannel<M: StreamChannelMeta>: Sendable {
    private let core: ChannelCore

    init(core: ChannelCore) {
        self.core = core
    }

    /// server → client の push event ストリーム。
    public var events: AsyncStream<M.Event> {
        // TODO(next pass): typed-frame dispatch を decode して M.Event を流す。
        // scaffold 段階では即終端の空ストリームを返す。
        AsyncStream { $0.finish() }
    }

    /// request を 1 本送り、 対応する response を待つ。
    public func request<R: UnisonRequest>(_ request: R) async throws -> R.Response {
        // TODO(next pass): R.method + payload を typed frame に乗せて送信し、
        // 同 id の response frame を await して R.Response に decode する。
        _ = request
        throw UnisonError.notImplemented("StreamChannel.request")
    }

    /// channel を閉じる。
    public func close() async {
        await core.close()
    }
}

/// datagram channel ハンドル (= unreliable な server push event 専用)。
public struct DatagramChannel<M: DatagramChannelMeta>: Sendable {
    private let core: ChannelCore

    init(core: ChannelCore) {
        self.core = core
    }

    /// server → client の push event ストリーム (QUIC datagram, RFC9221)。
    public var events: AsyncStream<M.Event> {
        // TODO(next pass): datagram demux → M.Event decode。
        AsyncStream { $0.finish() }
    }

    /// channel を閉じる。
    public func close() async {
        await core.close()
    }
}

/// channel の内部状態 (transport stream / datagram subscription の保持先)。
///
/// scaffold 段階では close のみ。 後続 pass で send/recv loop と frame dispatch を持つ。
actor ChannelCore {
    let name: String
    private var closed = false

    init(name: String) {
        self.name = name
    }

    func close() {
        closed = true
        // TODO(next pass): 紐づく QUIC stream / datagram subscription を tear down。
    }
}
