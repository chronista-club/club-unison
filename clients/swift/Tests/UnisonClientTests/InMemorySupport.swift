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
final class InMemoryStream: ChannelStream {
    private let sendInbox: InMemoryInbox
    private let readInbox: InMemoryInbox

    init(sendTo: InMemoryInbox, readFrom: InMemoryInbox) {
        self.sendInbox = sendTo
        self.readInbox = readFrom
    }

    func send(_ bytes: Data) async throws {
        await sendInbox.push(bytes)
    }

    func receive() async throws -> Data? {
        await readInbox.pop()
    }

    func close() async {
        await sendInbox.finish()
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
        pushEvents: [Data] = []
    ) -> Task<Void, Never> {
        Task {
            var reader = FrameReader()
            while let chunk = try? await stream.receive() {
                reader.append(chunk)
                while let body = nextBody(&reader) {
                    guard case let .protocolMessage(msg)? = try? Framing.decodeFrameBody(body) else {
                        continue
                    }
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
                        resp.payload = msg.payload // echo
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

    init(
        rejectOpen: Bool = false,
        pushEvents: [Data] = [],
        identityJSON: String = #"{"name":"stub","version":"1.0.0","namespace":"test","channels":[{"name":"ping-pong"}]}"#
    ) {
        self.rejectOpen = rejectOpen
        self.pushEvents = pushEvents
        self.identityJSON = identityJSON
    }

    func openStream() async throws -> any ChannelStream {
        let (client, server) = InMemoryPair.make()
        _ = EchoServerStub.serve(server, rejectOpen: rejectOpen, pushEvents: pushEvents)
        return client
    }

    func acceptStream() async throws -> (any ChannelStream)? {
        let (client, server) = InMemoryPair.make()
        _ = EchoServerStub.serveIdentity(server, json: identityJSON)
        return client
    }

    func close() async {}
}
