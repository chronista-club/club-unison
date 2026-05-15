//! フレームのシリアライゼーション/デシリアライゼーション
//!
//! UnisonPacket と Bytes の相互変換、 圧縮/解凍処理を実装する。
//!
//! ## v0.9.0 wire format
//!
//! ```text
//! [u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes (may be zstd compressed)]
//! ```
//!
//! - 先頭 4 byte は header bytes の長さ (big-endian u32)
//! - header 部は buffa (protobuf) でエンコードされた可変長
//! - payload 部の長さと圧縮状態は header の `payload_length` / `compressed_length` が
//!   表現する。 `compressed_length > 0` かつ `flags::COMPRESSED` が立っているとき
//!   zstd 圧縮されているとみなす。

use ::buffa::Message;
use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;
use zstd::stream::{decode_all, encode_all};

use super::{
    config::PacketConfig,
    flags::PacketFlags,
    header::UnisonPacketHeader,
};
use crate::proto;

/// シリアライゼーションエラー
#[derive(Error, Debug)]
pub enum SerializationError {
    #[error("Compression failed: {0}")]
    CompressionFailed(String),

    #[error("Decompression failed: {0}")]
    DecompressionFailed(String),

    #[error("Frame too large: {size} bytes (max: {max_size} bytes)")]
    PacketTooLarge { size: usize, max_size: usize },

    #[error("Invalid header")]
    InvalidHeader,

    #[error("Header length out of range: {0}")]
    HeaderLengthOutOfRange(u64),

    #[error("Incompatible protocol version: {version}")]
    IncompatibleVersion { version: u8 },

    #[error("Serialization failed: {0}")]
    SerializationFailed(String),

    #[error("Deserialization failed: {0}")]
    DeserializationFailed(String),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// フレームのシリアライゼーション処理
pub struct PacketSerializer;

impl PacketSerializer {
    /// ヘッダーとペイロードを Bytes に変換（デフォルト設定）
    pub fn serialize(
        header: &mut UnisonPacketHeader,
        payload: &[u8],
    ) -> Result<Bytes, SerializationError> {
        Self::serialize_with_config(header, payload, &PacketConfig::default())
    }

    /// ヘッダーとペイロードを Bytes に変換（カスタム設定）
    pub fn serialize_with_config(
        header: &mut UnisonPacketHeader,
        payload: &[u8],
        config: &PacketConfig,
    ) -> Result<Bytes, SerializationError> {
        let payload_size = payload.len();
        header.payload_length = payload_size as u32;

        // 圧縮判定と処理
        let (final_payload, is_compressed) = if config.compression.should_compress(payload_size) {
            let compressed = Self::compress(payload, config.compression.level)?;
            let compressed_size = compressed.len();

            // 圧縮が効果的な場合のみ使用
            if compressed_size < payload_size {
                header.compressed_length = compressed_size as u32;
                (compressed, true)
            } else {
                header.compressed_length = 0;
                (payload.to_vec(), false)
            }
        } else {
            header.compressed_length = 0;
            (payload.to_vec(), false)
        };

        // フラグを更新
        let mut flags = header.flags();
        if is_compressed {
            flags.set(PacketFlags::COMPRESSED);
        } else {
            flags.unset(PacketFlags::COMPRESSED);
        }
        header.set_flags(flags);

        // ヘッダーを buffa でエンコード
        let header_bytes = header.to_proto().encode_to_vec();
        let header_len = header_bytes.len();
        if header_len > u32::MAX as usize {
            return Err(SerializationError::HeaderLengthOutOfRange(header_len as u64));
        }

        // wire format: [u32 BE header_len] [header bytes] [payload bytes]
        let total_size = 4 + header_len + final_payload.len();
        if total_size > config.max_payload_size {
            return Err(SerializationError::PacketTooLarge {
                size: total_size,
                max_size: config.max_payload_size,
            });
        }

        let mut packet = BytesMut::with_capacity(total_size);
        packet.put_u32(header_len as u32);
        packet.put_slice(&header_bytes);
        packet.put_slice(&final_payload);

        Ok(packet.freeze())
    }

    /// ペイロードを圧縮
    fn compress(data: &[u8], level: i32) -> Result<Vec<u8>, SerializationError> {
        encode_all(data, level)
            .map_err(|e| SerializationError::CompressionFailed(e.to_string()))
    }
}

/// フレームのデシリアライゼーション処理
pub struct PacketDeserializer;

impl PacketDeserializer {
    /// パケットのヘッダーだけを取り出す (payload bytes 部分は touch しない)
    pub fn parse_header_only(
        bytes: &[u8],
    ) -> Result<UnisonPacketHeader, SerializationError> {
        if bytes.len() < 4 {
            return Err(SerializationError::InvalidHeader);
        }
        let header_len =
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + header_len {
            return Err(SerializationError::InvalidHeader);
        }

        let proto_header = proto::PacketHeader::decode_from_slice(&bytes[4..4 + header_len])
            .map_err(|e| SerializationError::DeserializationFailed(e.to_string()))?;
        let header = UnisonPacketHeader::from_proto(&proto_header);

        if !header.is_compatible() {
            return Err(SerializationError::IncompatibleVersion {
                version: header.version,
            });
        }
        Ok(header)
    }

    /// パケット全体をパースし、 ヘッダーと (必要なら解凍済みの) payload を返す
    pub fn parse(
        bytes: &[u8],
    ) -> Result<(UnisonPacketHeader, Vec<u8>), SerializationError> {
        if bytes.len() < 4 {
            return Err(SerializationError::InvalidHeader);
        }
        let header_len =
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + header_len {
            return Err(SerializationError::InvalidHeader);
        }

        let proto_header = proto::PacketHeader::decode_from_slice(&bytes[4..4 + header_len])
            .map_err(|e| SerializationError::DeserializationFailed(e.to_string()))?;
        let header = UnisonPacketHeader::from_proto(&proto_header);

        if !header.is_compatible() {
            return Err(SerializationError::IncompatibleVersion {
                version: header.version,
            });
        }

        let payload_bytes = &bytes[4 + header_len..];
        let expected_size = header.actual_payload_size() as usize;
        if payload_bytes.len() != expected_size {
            return Err(SerializationError::InvalidHeader);
        }

        let payload = if header.is_compressed() {
            decode_all(payload_bytes)
                .map_err(|e| SerializationError::DecompressionFailed(e.to_string()))?
        } else {
            payload_bytes.to_vec()
        };

        Ok((header, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::PacketType;

    #[test]
    fn test_serialize_small_packet() {
        // 圧縮閾値未満のフレーム
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        let payload = b"Hello, World!";

        let packet = PacketSerializer::serialize(&mut header, payload).unwrap();

        assert!(!header.is_compressed());
        assert_eq!(header.compressed_length, 0);
        // 先頭 4 byte が header_len、 後ろが header + payload なので
        // パケット全体は最低でも 4 + (何らかのサイズ) + payload.len() より大きい
        assert!(packet.len() > 4 + payload.len());
    }

    #[test]
    fn test_serialize_large_packet() {
        // 圧縮閾値以上のフレーム
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        let large_text = "x".repeat(3000);
        let payload = large_text.as_bytes();

        let _packet = PacketSerializer::serialize(&mut header, payload).unwrap();

        assert!(header.is_compressed());
        assert!(header.compressed_length > 0);
        assert!(header.compressed_length < header.payload_length);
    }

    #[test]
    fn test_round_trip() {
        let mut header = UnisonPacketHeader::new(PacketType::Data)
            .with_sequence(42)
            .with_stream_id(1337);

        let payload = b"Test payload data";

        // シリアライズ
        let packet = PacketSerializer::serialize(&mut header, payload).unwrap();

        // デシリアライズ
        let (restored_header, restored_payload) = PacketDeserializer::parse(&packet).unwrap();

        assert_eq!(restored_header.sequence_number, 42);
        assert_eq!(restored_header.stream_id, 1337);
        assert_eq!(restored_payload, payload);
    }

    #[test]
    fn test_compression_effectiveness() {
        // 圧縮が効果的なデータ
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        let repetitive_data = "a".repeat(3000);
        let payload = repetitive_data.as_bytes();

        let _packet = PacketSerializer::serialize(&mut header, payload).unwrap();

        assert!(header.is_compressed());
        assert!(header.compressed_length < header.payload_length / 2);
    }

    #[test]
    fn test_parse_header_only_skips_payload() {
        let mut header = UnisonPacketHeader::new(PacketType::Data)
            .with_message_id(99)
            .with_response_to(7);
        let payload = b"payload bytes";

        let packet = PacketSerializer::serialize(&mut header, payload).unwrap();
        let header_only = PacketDeserializer::parse_header_only(&packet).unwrap();

        assert_eq!(header_only.message_id, 99);
        assert_eq!(header_only.response_to, 7);
        assert_eq!(header_only.payload_length, payload.len() as u32);
    }
}
