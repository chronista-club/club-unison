import Foundation
import SwiftProtobuf

/// Channel stream の wire frame。 Rust `quic.rs` の `read_typed_frame` /
/// `write_typed_frame` と byte 一致する layout:
///
/// ```text
/// [4B BE total_len] [1B frame_type] [payload]
/// ```
///
/// - `total_len` = frame_type (1B) + payload
/// - `frame_type` = 0x00 PROTOCOL (= UnisonPacket) / 0x01 RAW (= 生 bytes)
/// - PROTOCOL frame の payload = UnisonPacket (`[u32 header_len][PacketHeader][ProtocolMessage]`)
enum Framing {
    static let frameTypeProtocol: UInt8 = 0x00
    static let frameTypeRaw: UInt8 = 0x01

    /// typed frame の最大 size (= Rust `MAX_MESSAGE_SIZE`、 8MB)。
    static let maxFrameSize = 8 * 1024 * 1024

    /// `ProtocolMessage` を 1 本の PROTOCOL typed frame へ encode する。
    static func encodeProtocolFrame(
        _ message: Protocol_ProtocolMessage,
        packetOptions: Packet.Options = Packet.Options()
    ) throws -> Data {
        let msgBytes: Data
        do {
            msgBytes = try message.serializedData()
        } catch {
            throw UnisonError.codec("ProtocolMessage encode 失敗: \(error)")
        }
        let packet = try Packet.encode(payload: msgBytes, options: packetOptions)
        return wrap(frameType: frameTypeProtocol, payload: packet)
    }

    /// 生 byte 列を 1 本の RAW typed frame へ encode する。
    static func encodeRawFrame(_ data: Data) -> Data {
        wrap(frameType: frameTypeRaw, payload: data)
    }

    /// type tag + payload を length-prefixed typed frame へ wrap する。
    static func wrap(frameType: UInt8, payload: Data) -> Data {
        let totalLen = 1 + payload.count
        var frame = Data(capacity: 4 + totalLen)
        frame.appendBigEndian(UInt32(totalLen))
        frame.append(frameType)
        frame.append(payload)
        return frame
    }

    /// typed frame の decode 結果。
    enum Decoded {
        case protocolMessage(Protocol_ProtocolMessage)
        case raw(Data)
    }

    /// 1 本の typed frame body (= 4B length prefix を剥がした後の `[1B type][payload]`)
    /// を decode する。
    static func decodeFrameBody(_ body: Data) throws -> Decoded {
        guard let frameType = body.first else {
            throw UnisonError.codec("frame: 空の typed frame (type tag 欠落)")
        }
        let payload = Data(body[(body.startIndex + 1)...])
        switch frameType {
        case frameTypeProtocol:
            let packet = try Packet.decode(payload)
            do {
                let message = try Protocol_ProtocolMessage(serializedBytes: packet.payload)
                return .protocolMessage(message)
            } catch {
                throw UnisonError.codec("ProtocolMessage decode 失敗: \(error)")
            }
        case frameTypeRaw:
            return .raw(payload)
        default:
            throw UnisonError.codec("frame: 未知の frame type tag 0x\(String(frameType, radix: 16))")
        }
    }
}

/// byte stream から typed frame body を 1 本ずつ取り出す incremental パーサ。
///
/// QUIC stream の chunk を `append` で投入し、 揃った frame body を `nextFrame`
/// で取り出す。 frame 跨ぎの chunk は内部 buffer で結合する (= Rust / TS の
/// `read_typed_frame` / `readFrames` と同じ境界処理)。
struct FrameReader {
    private var buffer = Data()

    mutating func append(_ chunk: Data) {
        buffer.append(chunk)
    }

    /// 揃っていれば次の frame body (= `[1B type][payload]`) を返す。 未到達なら nil。
    mutating func nextFrame() throws -> Data? {
        guard buffer.count >= 4 else { return nil }
        let totalLen = Int(buffer.readBigEndianUInt32(at: buffer.startIndex))
        if totalLen == 0 {
            throw UnisonError.codec("frame: 空 frame (total_len = 0)")
        }
        if totalLen > Framing.maxFrameSize {
            throw UnisonError.codec("frame: too large (\(totalLen) bytes)")
        }
        guard buffer.count >= 4 + totalLen else { return nil }
        let bodyStart = buffer.startIndex + 4
        let body = Data(buffer[bodyStart..<(bodyStart + totalLen)])
        buffer.removeSubrange(buffer.startIndex..<(bodyStart + totalLen))
        return body
    }
}
