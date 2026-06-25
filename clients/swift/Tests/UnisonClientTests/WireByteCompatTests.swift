import Foundation
import Testing
@testable import UnisonClient

/// wire byte-compat: Swift encoder == Rust golden fixture。
///
/// fixture は `crates/unison-protocol/tests/fixtures/wire/` を test resource に
/// コピーしたもの (= TS の byte_compat.test.ts と同じ Rust 出力)。 これに byte
/// 一致することが、 quinn server / 他 client との interop の生命線。
struct WireByteCompatTests {
    /// fixture hex を Data へ。
    private func fixture(_ name: String) throws -> Data {
        let url = try #require(
            Bundle.module.url(forResource: name, withExtension: "hex", subdirectory: "Fixtures/wire"),
            "fixture \(name).hex が見つからない"
        )
        let hex = try String(contentsOf: url, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return Data(hexString: hex)
    }

    // MARK: encode: Swift frame == Rust fixture

    @Test func requestFrameMatchesRustFixture() throws {
        var msg = Protocol_ProtocolMessage()
        msg.id = 7
        msg.method = "SubscribeMetric"
        msg.msgType = .request
        msg.payload = Data(#"{"names":["cpu","memory"]}"#.utf8)

        let frame = try Framing.encodeProtocolFrame(msg)
        #expect(frame.hexString == (try fixture("request_frame")).hexString)
    }

    @Test func eventFrameMatchesRustFixture() throws {
        var msg = Protocol_ProtocolMessage()
        msg.id = 0
        msg.method = "MetricUpdate"
        msg.msgType = .event
        msg.payload = Data(#"{"name":"cpu","value":42}"#.utf8)

        let frame = try Framing.encodeProtocolFrame(msg)
        #expect(frame.hexString == (try fixture("event_frame")).hexString)
    }

    // MARK: decode: Rust fixture → 期待する ProtocolMessage

    @Test func decodesRustRequestFrame() throws {
        let body = stripLengthPrefix(try fixture("request_frame"))
        guard case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) else {
            Issue.record("PROTOCOL frame を期待")
            return
        }
        #expect(msg.id == 7)
        #expect(msg.method == "SubscribeMetric")
        #expect(msg.msgType == .request)
    }

    @Test func decodesOpenAckFrame() throws {
        let body = stripLengthPrefix(try fixture("open_ack_frame"))
        guard case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) else {
            Issue.record("PROTOCOL frame を期待")
            return
        }
        #expect(msg.method == "__channel_ack")
        #expect(msg.msgType == .response)
    }

    @Test func decodesOpenNackFrame() throws {
        let body = stripLengthPrefix(try fixture("open_nack_frame"))
        guard case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) else {
            Issue.record("PROTOCOL frame を期待")
            return
        }
        #expect(msg.method == "__channel_ack")
        #expect(msg.msgType == .error)
    }

    @Test func decodesIdentityFrame() throws {
        let body = stripLengthPrefix(try fixture("identity_frame"))
        guard case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) else {
            Issue.record("PROTOCOL frame を期待")
            return
        }
        #expect(msg.method == "__identity")
    }

    // MARK: FrameReader: chunk 跨ぎ結合

    @Test func frameReaderReassemblesAcrossChunks() throws {
        let frame = try fixture("request_frame")
        var reader = FrameReader()
        // 1 byte ずつ投入しても 1 本の frame に結合されること。
        var produced: [Data] = []
        for byte in frame {
            reader.append(Data([byte]))
            while let body = try reader.nextFrame() { produced.append(body) }
        }
        #expect(produced.count == 1)
        if let body = produced.first {
            guard case let .protocolMessage(msg) = try Framing.decodeFrameBody(body) else {
                Issue.record("PROTOCOL frame を期待")
                return
            }
            #expect(msg.id == 7)
        }
    }

    @Test func frameReaderSplitsTwoConcatenatedFrames() throws {
        var concatenated = try fixture("request_frame")
        concatenated.append(try fixture("event_frame"))
        var reader = FrameReader()
        reader.append(concatenated)
        var count = 0
        while try reader.nextFrame() != nil { count += 1 }
        #expect(count == 2)
    }

    /// 4B length prefix を剥がして frame body (`[1B type][payload]`) を返す。
    private func stripLengthPrefix(_ frame: Data) -> Data {
        Data(frame[(frame.startIndex + 4)...])
    }
}

extension Data {
    /// hex 文字列 → Data。
    init(hexString: String) {
        var data = Data(capacity: hexString.count / 2)
        var idx = hexString.startIndex
        while idx < hexString.endIndex {
            let next = hexString.index(idx, offsetBy: 2)
            if let byte = UInt8(hexString[idx..<next], radix: 16) {
                data.append(byte)
            }
            idx = next
        }
        self = data
    }

    /// Data → lowercase hex 文字列。
    var hexString: String {
        map { String(format: "%02x", $0) }.joined()
    }
}
