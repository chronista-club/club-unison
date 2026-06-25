import Foundation

/// 接続ライフサイクルの event。 reconnect は library ではなく caller の責務
/// (= TS と同方針)。 caller は `disconnected` を観測して再接続戦略を回す。
public enum ConnectionEvent: Sendable, Equatable {
    /// QUIC handshake 完了、 接続確立。
    case connected
    /// 接続切断。 reason は判明していれば付く。
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

    init(transport: any ChannelTransport) {
        self.transport = transport
        let (stream, sink) = AsyncStream<ConnectionEvent>.makeStream()
        self.events = stream
        self.eventSink = sink
        sink.yield(.connected)
    }

    /// 接続ライフサイクル event ストリーム。
    public nonisolated var connectionEvents: AsyncStream<ConnectionEvent> {
        events
    }

    /// server の自己記述を取得する (= identity stream を accept して読む)。
    public func serverIdentity() async throws -> ServerIdentity {
        guard let stream = try await transport.acceptStream() else {
            throw UnisonError.notConnected
        }
        return try await IdentityHandshake.read(from: stream)
    }

    /// stream channel を開く (= 新 stream → `__channel:{name}` open → open_ack 待ち)。
    public func openChannel<M: StreamChannelMeta>(_ meta: M) async throws -> StreamChannel<M> {
        _ = meta
        let stream = try await transport.openStream()
        let core = StreamChannelCore(name: M.name, stream: stream)
        await core.start()
        try await core.open()
        return StreamChannel(core: core)
    }

    /// datagram channel を開く。
    public func openDatagramChannel<M: DatagramChannelMeta>(_ meta: M) async throws -> DatagramChannel<M> {
        _ = meta
        // TODO(next pass): datagram channel 登録 + channelId 紐づけ (QUIC datagram)。
        return DatagramChannel(name: M.name)
    }

    /// 接続を切断する。
    public func disconnect() async {
        await transport.close()
        eventSink.yield(.disconnected(reason: nil))
        eventSink.finish()
    }
}
