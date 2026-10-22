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

use buffa::Message;
use bytes::{BufMut, Bytes, BytesMut};
use thiserror::Error;
use zstd::bulk::decompress;
use zstd::stream::encode_all;

use super::{config::PacketConfig, flags::PacketFlags, header::UnisonPacketHeader};
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

    #[error(
        "Decompressed payload size mismatch: header claims {claimed} bytes but got {actual} bytes"
    )]
    DecompressedSizeMismatch { claimed: usize, actual: usize },

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
            return Err(SerializationError::HeaderLengthOutOfRange(
                header_len as u64,
            ));
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
        encode_all(data, level).map_err(|e| SerializationError::CompressionFailed(e.to_string()))
    }
}

/// フレームのデシリアライゼーション処理
pub struct PacketDeserializer;

impl PacketDeserializer {
    /// パケットのヘッダーだけを取り出す (payload bytes 部分は touch しない)
    pub fn parse_header_only(bytes: &[u8]) -> Result<UnisonPacketHeader, SerializationError> {
        if bytes.len() < 4 {
            return Err(SerializationError::InvalidHeader);
        }
        let header_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
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
    /// （デフォルト設定）
    pub fn parse(bytes: &[u8]) -> Result<(UnisonPacketHeader, Vec<u8>), SerializationError> {
        Self::parse_with_config(bytes, &PacketConfig::default())
    }

    /// パケット全体をパースし、 ヘッダーと (必要なら解凍済みの) payload を返す
    /// （カスタム設定）
    ///
    /// 圧縮 payload は、 header が申告する解凍後サイズ (`config.max_payload_size`
    /// 以下) の出力バッファにだけ解凍し、 超えた時点で打ち切る。 これにより
    /// 小さな圧縮フレームが GB 級に膨張する zstd decompression bomb を防ぐ。
    pub fn parse_with_config(
        bytes: &[u8],
        config: &PacketConfig,
    ) -> Result<(UnisonPacketHeader, Vec<u8>), SerializationError> {
        if bytes.len() < 4 {
            return Err(SerializationError::InvalidHeader);
        }
        let header_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
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
            // 解凍後サイズが膨張するのを防ぐため、まず header が主張する
            // 非圧縮サイズ (payload_length) を上限チェックする。
            let claimed = header.payload_length as usize;
            if claimed > config.max_payload_size {
                return Err(SerializationError::PacketTooLarge {
                    size: claimed,
                    max_size: config.max_payload_size,
                });
            }

            // zstd decompression bomb 対策: 出力を申告サイズで打ち切る。 全量を
            // 展開してから測ると、 検査の前に任意量のメモリを確保させられる。
            let decompressed = decompress(payload_bytes, claimed)
                .map_err(|e| SerializationError::DecompressionFailed(e.to_string()))?;

            // 申告より短く終わった場合を弾く。
            if decompressed.len() != claimed {
                return Err(SerializationError::DecompressedSizeMismatch {
                    claimed,
                    actual: decompressed.len(),
                });
            }

            decompressed
        } else {
            // 非圧縮 path も compressed path と対称に max_payload_size で上限チェック
            // する (= でないと巨大な非圧縮 payload が制限を素通りする)。
            if payload_bytes.len() > config.max_payload_size {
                return Err(SerializationError::PacketTooLarge {
                    size: payload_bytes.len(),
                    max_size: config.max_payload_size,
                });
            }
            payload_bytes.to_vec()
        };

        Ok((header, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::PacketType;
    use crate::packet::config::{CompressionConfig, PacketConfig};
    use std::io::Read;

    #[test]
    fn deserialize_uncompressed_payload_bounded_by_max() {
        // 圧縮無効 + 大きい max で serialize (= 非圧縮 packet を作る)
        let big = PacketConfig::new()
            .with_compression(CompressionConfig::disabled())
            .with_max_payload_size(1024 * 1024);
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        let payload = vec![0u8; 4096];
        let packet = PacketSerializer::serialize_with_config(&mut header, &payload, &big).unwrap();
        assert!(!header.is_compressed());

        // 小さい max で deserialize → 非圧縮 path の上限チェックに当たるべき
        let small = PacketConfig::new()
            .with_compression(CompressionConfig::disabled())
            .with_max_payload_size(1024);
        let err = PacketDeserializer::parse_with_config(&packet, &small).unwrap_err();
        assert!(
            matches!(err, SerializationError::PacketTooLarge { .. }),
            "非圧縮 payload も max_payload_size で bound されるべき: {err:?}"
        );
    }

    /// header を手で組み、 任意の申告サイズと圧縮 payload を持つ packet を作る
    fn crafted_compressed_packet(claimed: u32, compressed: &[u8]) -> Vec<u8> {
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        header.payload_length = claimed;
        header.compressed_length = compressed.len() as u32;
        let mut flags = header.flags();
        flags.set(PacketFlags::COMPRESSED);
        header.set_flags(flags);
        let header_bytes = header.to_proto().encode_to_vec();

        let mut packet = Vec::new();
        packet.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        packet.extend_from_slice(&header_bytes);
        packet.extend_from_slice(compressed);
        packet
    }

    #[test]
    fn deserialize_stops_decompressing_at_claimed_size() {
        // 64MB の 0 は数 KB に縮む。 header は 16 byte と申告する (= decompression bomb)
        let expanded = 64 * 1024 * 1024;
        let bomb = encode_all(std::io::repeat(0).take(expanded), 3).unwrap();
        let packet = crafted_compressed_packet(16, &bomb);

        let err = PacketDeserializer::parse(&packet).unwrap_err();
        // 申告の 16 byte を超えた時点で解凍を打ち切るべき。 全量を展開してから
        // 検査すると、 小さなフレームひとつで任意量のメモリを確保させられる
        assert!(
            matches!(err, SerializationError::DecompressionFailed(_)),
            "申告サイズを超えた時点で解凍を止めるべき: {err:?}"
        );
    }

    #[test]
    fn deserialize_rejects_decompressed_shorter_than_claimed() {
        let compressed = encode_all(&[7u8; 100][..], 3).unwrap();
        let packet = crafted_compressed_packet(200, &compressed);

        let err = PacketDeserializer::parse(&packet).unwrap_err();
        assert!(
            matches!(
                err,
                SerializationError::DecompressedSizeMismatch {
                    claimed: 200,
                    actual: 100
                }
            ),
            "申告より短い解凍結果は弾くべき: {err:?}"
        );
    }

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
        let mut header = UnisonPacketHeader::new(PacketType::Control);
        header.timestamp = 12345;

        let payload = b"Test payload data";

        // シリアライズ
        let packet = PacketSerializer::serialize(&mut header, payload).unwrap();

        // デシリアライズ
        let (restored_header, restored_payload) = PacketDeserializer::parse(&packet).unwrap();

        assert_eq!(restored_header.packet_type(), Ok(PacketType::Control));
        assert_eq!(restored_header.timestamp, 12345);
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
        let mut header = UnisonPacketHeader::new(PacketType::Control);
        header.timestamp = 99;
        let payload = b"payload bytes";

        let packet = PacketSerializer::serialize(&mut header, payload).unwrap();
        let header_only = PacketDeserializer::parse_header_only(&packet).unwrap();

        assert_eq!(header_only.packet_type(), Ok(PacketType::Control));
        assert_eq!(header_only.timestamp, 99);
        assert_eq!(header_only.payload_length, payload.len() as u32);
    }
}
