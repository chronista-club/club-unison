import Foundation
@testable import UnisonClient

/// 片方向 byte queue。 push / pop を continuation で繋ぐ (= AsyncIterator の actor
/// 制約を避けるための最小実装)。
actor InMemoryInbox {
    private var buffer: [Data] = []
    private var finished = false
    private var waiter: CheckedContinuation<Data?, Never>?

    func push(_ data: Data) {
        if let w = waiter {
            waiter = nil
            w.resume(returning: data)
        } else {
            buffer.append(data)
        }
    }

    func finish() {
        finished = true
        if let w = waiter {
            waiter = nil
            w.resume(returning: nil)
        }
    }

    func pop() async -> Data? {
        if !buffer.isEmpty { return buffer.removeFirst() }
        if finished { return nil }
        return await withCheckedContinuation { cont in waiter = cont }
    }
}

/// in-memory な双方向 stream。 2 endpoint を memory pipe で繋ぐ (= TS `MockTransport`
/// の Swift 版)。 NWProtocolQUIC 抜きで channel ロジックを決定論的にテストする。
final class InMemoryStream: ChannelStream, @unchecked Sendable {
    private let sendInbox: InMemoryInbox
    private let readInbox: InMemoryInbox
    private let lock = NSLock()
    private var closes = 0

    init(sendTo: InMemoryInbox, readFrom: InMemoryInbox) {
        self.sendInbox = sendTo
        self.readInbox = readFrom
    }

    /// `close()` が呼ばれた回数 (= 後始末のテスト用)。
    var closeCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return closes
    }

    func send(_ bytes: Data) async throws {
        await sendInbox.push(bytes)
    }

    func receive() async throws -> Data? {
        await readInbox.pop()
    }

    /// 閉じると相手は EOF を読み、 自分の receive も EOF で戻る
    /// (= NWConnection.cancel() が自分の receive も終わらせるのに合わせる)。
    func close() async {
        lock.withLock { closes += 1 }
        await sendInbox.finish()
        await readInbox.finish()
    }
}

/// client / server 両端の `InMemoryStream` ペアを作る。
enum InMemoryPair {
    static func make() -> (client: InMemoryStream, server: InMemoryStream) {
        let clientToServer = InMemoryInbox()
        let serverToClient = InMemoryInbox()
        let client = InMemoryStream(sendTo: clientToServer, readFrom: serverToClient)
        let server = InMemoryStream(sendTo: serverToClient, readFrom: clientToServer)
        return (client, server)
    }
}

/// server 端の最小スタブ (= TS `StreamServerStub` の Swift 版)。
///
/// - `__channel:` open → `__channel_ack` (response = accept / `rejectOpen` 時は error)
/// - request → echo (= 同 payload を response で返す)
/// - open 直後に `pushEvents` を event frame として流す
enum EchoServerStub {
    static func serve(
        _ stream: any ChannelStream,
        rejectOpen: Bool = false,
        pushEvents: [Data] = [],
        silent: Set<String> = [],
        respond: (@Sendable (String, Data) -> Data?)? = nil
    ) -> Task<Void, Never> {
        Task {
            var reader = FrameReader()
            while let chunk = try? await stream.receive() {
                reader.append(chunk)
                while let body = nextBody(&reader) {
                    guard case let .protocolMessage(msg)? = try? Framing.decodeFrameBody(body) else {
                        continue
                    }
                    // silent に入っている method には何も返さない (= 固まった server)。
                    if silent.contains(msg.method) { continue }
                    if msg.method.hasPrefix("__channel:") {
                        var ack = Protocol_ProtocolMessage()
                        ack.id = msg.id
                        ack.method = "__channel_ack"
                        ack.msgType = rejectOpen ? .error : .response
                        if rejectOpen {
                            ack.payload = Data(#"{"error":"channel-not-found"}"#.utf8)
                        }
                        await sendFrame(stream, ack)
                        if !rejectOpen {
                            for ev in pushEvents {
                                var event = Protocol_ProtocolMessage()
                                event.method = "event"
                                event.msgType = .event
                                event.payload = ev
                                await sendFrame(stream, event)
                            }
                        }
                    } else if msg.msgType == .request {
                        var resp = Protocol_ProtocolMessage()
                        resp.id = msg.id
                        resp.method = msg.method
                        resp.msgType = .response
                        // respond クロージャがあればその応答、 無ければ echo (= 同 payload)。
                        resp.payload = respond?(msg.method, msg.payload) ?? msg.payload
                        await sendFrame(stream, resp)
                    }
                }
            }
        }
    }

    /// identity stream を 1 本 push して finish する server 端。
    static func serveIdentity(_ stream: any ChannelStream, json: String) -> Task<Void, Never> {
        Task {
            var msg = Protocol_ProtocolMessage()
            msg.method = "__identity"
            msg.msgType = .event
            msg.payload = Data(json.utf8)
            await sendFrame(stream, msg)
            await stream.close()
        }
    }

    private static func nextBody(_ reader: inout FrameReader) -> Data? {
        (try? reader.nextFrame()) ?? nil
    }

    private static func sendFrame(_ stream: any ChannelStream, _ msg: Protocol_ProtocolMessage) async {
        if let frame = try? Framing.encodeProtocolFrame(msg) {
            try? await stream.send(frame)
        }
    }
}

/// client 側 transport。 openStream ごとに echo stub を server 端へ配する。
final class StubTransport: ChannelTransport, @unchecked Sendable {
    let rejectOpen: Bool
    let pushEvents: [Data]
    let identityJSON: String
    let respond: (@Sendable (String, Data) -> Data?)?

    init(
        rejectOpen: Bool = false,
        pushEvents: [Data] = [],
        identityJSON: String = #"{"name":"stub","version":"1.0.0","namespace":"test","channels":[{"name":"ping-pong"}]}"#,
        respond: (@Sendable (String, Data) -> Data?)? = nil
    ) {
        self.rejectOpen = rejectOpen
        self.pushEvents = pushEvents
        self.identityJSON = identityJSON
        self.respond = respond
    }

    func openStream() async throws -> any ChannelStream {
        let (client, server) = InMemoryPair.make()
        _ = EchoServerStub.serve(server, rejectOpen: rejectOpen, pushEvents: pushEvents, respond: respond)
        return client
    }

    func acceptStream() async throws -> (any ChannelStream)? {
        let (client, server) = InMemoryPair.make()
        _ = EchoServerStub.serveIdentity(server, json: identityJSON)
        return client
    }

    func close() async {}
}

/// テストから server 端を操作できる transport。 openStream ごとに echo stub を配し、
/// client / server 両端の stream を控えておく。
final class ControlledTransport: ChannelTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var clients: [InMemoryStream] = []
    private var servers: [InMemoryStream] = []
    private var closes = 0
    private let rejectOpen: Bool
    private let pushEvents: [Data]
    private let silent: Set<String>
    private let incoming = PendingStreams()
    private var closeHandler: (@Sendable (String?) -> Void)?

    init(rejectOpen: Bool = false, pushEvents: [Data] = [], silent: Set<String> = []) {
        self.rejectOpen = rejectOpen
        self.pushEvents = pushEvents
        self.silent = silent
    }

    var clientStreams: [InMemoryStream] { lock.withLock { clients } }
    var serverStreams: [InMemoryStream] { lock.withLock { servers } }
    var closeCount: Int { lock.withLock { closes } }

    func openStream() async throws -> any ChannelStream {
        let (client, server) = InMemoryPair.make()
        lock.withLock {
            clients.append(client)
            servers.append(server)
        }
        _ = EchoServerStub.serve(server, rejectOpen: rejectOpen, pushEvents: pushEvents, silent: silent)
        return client
    }

    /// server 起点の stream。 `pushIdentity` を呼ぶまで来ない (= identity を送らない server)。
    func acceptStream() async throws -> (any ChannelStream)? {
        await incoming.pop()
    }

    /// server が identity stream を開いて送ってくる。 client 端を返す。
    @discardableResult
    func pushIdentity(
        json: String = #"{"name":"stub","version":"1.0.0","namespace":"test","channels":[]}"#
    ) async -> InMemoryStream {
        let (client, server) = InMemoryPair.make()
        _ = EchoServerStub.serveIdentity(server, json: json)
        await incoming.push(client)
        return client
    }

    func onClosed(_ handler: @escaping @Sendable (String?) -> Void) async {
        lock.withLock { closeHandler = handler }
    }

    /// 相手からの切断 / 経路の失敗を transport が検知した、 という状況を作る。
    /// handler が登録されるまで待ってから呼ぶ。
    func simulateRemoteClose(reason: String?) async {
        _ = await eventually { self.lock.withLock { self.closeHandler != nil } }
        let handler = lock.withLock { closeHandler }
        handler?(reason)
    }

    func close() async {
        lock.withLock { closes += 1 }
        for stream in clientStreams {
            await stream.close()
        }
        await incoming.finish()
    }
}

struct TestTimedOut: Error {}

/// `operation` が `seconds` 以内に終わらなければ `TestTimedOut` を投げる。
/// 終わらない operation は置き去りにする (= cancel に応じない待ちをテストで検出するため、
/// task group では待たない)。
func finishes<T: Sendable>(
    within seconds: Double = 2,
    _ operation: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withCheckedThrowingContinuation { cont in
        let once = ResumeOnce()
        Task {
            do {
                let value = try await operation()
                if once.tryResume() { cont.resume(returning: value) }
            } catch {
                if once.tryResume() { cont.resume(throwing: error) }
            }
        }
        Task {
            try? await Task.sleep(for: .seconds(seconds))
            if once.tryResume() { cont.resume(throwing: TestTimedOut()) }
        }
    }
}

/// `condition` が真になるまで待つ (最大 `seconds`)。 真にならなければ false。
func eventually(within seconds: Double = 2, _ condition: @Sendable () async -> Bool) async -> Bool {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        if await condition() { return true }
        try? await Task.sleep(for: .milliseconds(10))
    }
    return await condition()
}
