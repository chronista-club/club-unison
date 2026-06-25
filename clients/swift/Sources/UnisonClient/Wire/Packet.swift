import Foundation
import SwiftProtobuf

/// `UnisonPacket` wire layer。 Rust `crates/unison-protocol/src/packet/` の
/// `[u32 BE header_len][PacketHeader][payload]` と byte 一致する。
///
/// ```text
/// [u32 BE header_len] [proto3 PacketHeader] [payload bytes]
/// ```
///
/// `payload` は encode 済み `ProtocolMessage` バイト列。 TS client と同様、 常に
/// 非圧縮で送る (`compressed_length = 0`)。 圧縮 packet 受信時は明示 error。
enum Packet {
    /// PacketHeader に乗せる任意設定 (= 全 field 既定、 caller が必要分のみ上書き)。
    struct Options {
        var packetType: UInt32 = 0
        var sequenceNumber: UInt64 = 0
        var streamID: UInt64 = 0
        var messageID: UInt64 = 0
        var responseTo: UInt64 = 0
        var correlationID: Data = Data()
    }

    /// プロトコルバージョン (= Rust `UnisonPacketHeader::CURRENT_VERSION`)。
    static let version: UInt32 = 1

    /// `payload` (= encode 済み `ProtocolMessage`) を 1 本の UnisonPacket へ encode。
    static func encode(payload: Data, options: Options = Options()) throws -> Data {
        var header = Protocol_PacketHeader()
        header.version = version
        header.packetType = options.packetType
        header.payloadLength = UInt32(payload.count)
        header.compressedLength = 0
        header.sequenceNumber = options.sequenceNumber
        header.streamID = options.streamID
        header.messageID = options.messageID
        header.responseTo = options.responseTo
        header.correlationID = options.correlationID

        let headerBytes: Data
        do {
            headerBytes = try header.serializedData()
        } catch {
            throw UnisonError.codec("PacketHeader encode 失敗: \(error)")
        }

        var out = Data(capacity: 4 + headerBytes.count + payload.count)
        out.appendBigEndian(UInt32(headerBytes.count))
        out.append(headerBytes)
        out.append(payload)
        return out
    }

    /// decode 結果 (= header + 非圧縮 payload バイト列)。
    struct Decoded {
        let header: Protocol_PacketHeader
        let payload: Data
    }

    /// UnisonPacket バイト列を header + payload に分解する。
    static func decode(_ bytes: Data) throws -> Decoded {
        guard bytes.count >= 4 else {
            throw UnisonError.codec("packet: u32 header_len prefix に足りない")
        }
        let headerLen = Int(bytes.readBigEndianUInt32(at: bytes.startIndex))
        guard bytes.count >= 4 + headerLen else {
            throw UnisonError.codec("packet: header_len がバッファを超過")
        }
        let headerStart = bytes.startIndex + 4
        let headerBytes = bytes[headerStart..<(headerStart + headerLen)]
        let payload = Data(bytes[(headerStart + headerLen)...])

        let header: Protocol_PacketHeader
        do {
            header = try Protocol_PacketHeader(serializedBytes: Data(headerBytes))
        } catch {
            throw UnisonError.codec("PacketHeader decode 失敗: \(error)")
        }

        let expected = header.compressedLength > 0
            ? Int(header.compressedLength)
            : Int(header.payloadLength)
        guard payload.count == expected else {
            throw UnisonError.codec(
                "packet: payload size \(payload.count) != header 宣言 \(expected)"
            )
        }
        if header.compressedLength > 0 && (header.flags & 0x0001) != 0 {
            throw UnisonError.codec(
                "packet: zstd 圧縮 payload は未対応 (= fixture は < 2KB / 非圧縮)"
            )
        }
        return Decoded(header: header, payload: payload)
    }
}

extension Data {
    /// big-endian u32 を末尾に追記。
    mutating func appendBigEndian(_ value: UInt32) {
        append(UInt8((value >> 24) & 0xff))
        append(UInt8((value >> 16) & 0xff))
        append(UInt8((value >> 8) & 0xff))
        append(UInt8(value & 0xff))
    }

    /// 指定 offset から big-endian u32 を読む。
    func readBigEndianUInt32(at index: Index) -> UInt32 {
        let b0 = UInt32(self[index])
        let b1 = UInt32(self[index + 1])
        let b2 = UInt32(self[index + 2])
        let b3 = UInt32(self[index + 3])
        return (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    }
}
