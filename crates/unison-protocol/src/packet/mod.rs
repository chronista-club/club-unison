//! # UnisonPacket — バイナリフレームフォーマット
//!
//! Unison Protocol で使用される wire-level frame 表現。
//!
//! ## v0.9.0 wire format
//!
//! ```text
//! [u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes (may be zstd compressed)]
//! ```
//!
//! - 旧 v0.8 系の rkyv 56-byte fixed header は廃止
//! - header は buffa (protobuf) でシリアライズされた可変長
//! - payload は任意の codec (= buffa / JSON / raw bytes) で encode された Vec<u8>
//! - 2KB 以上の payload は自動で zstd 圧縮 (フラグで判別)
//!
//! ## 使用例
//!
//! ```ignore
//! use club_unison::packet::{UnisonPacket, PacketType};
//!
//! // 任意の payload bytes (= caller が codec で encode 済み)
//! let payload: Vec<u8> = b"Hello, World!".to_vec();
//!
//! let packet = UnisonPacket::builder()
//!     .with_stream_id(123)
//!     .with_sequence(1)
//!     .build(payload)?;
//!
//! // Bytes に変換（ネットワーク送信用）
//! let bytes = packet.to_bytes();
//!
//! // Bytes から復元
//! let restored = UnisonPacket::from_bytes(&bytes)?;
//! ```

pub mod config;
pub mod flags;
pub mod header;
pub mod serialization;

// 主要な型を再エクスポート
pub use config::{CompressionConfig, PacketConfig};
pub use flags::PacketFlags;
pub use header::{PacketType, UnisonPacketHeader};
pub use serialization::{PacketDeserializer, PacketSerializer, SerializationError};

use bytes::Bytes;

/// UnisonPacket — 生のシリアライズ済みフレーム
///
/// `[u32 BE header_len][buffa-encoded PacketHeader][payload bytes]` の
/// バイト列を保持する。 payload は caller が任意の codec で encode した
/// `Vec<u8>` (= rkyv 時代の generic `Payloadable` は廃止)。
pub struct UnisonPacket {
    /// シリアライズされたフレームデータ
    raw_data: Bytes,
}

impl UnisonPacket {
    /// フレームビルダーを作成
    pub fn builder() -> UnisonPacketBuilder {
        UnisonPacketBuilder::new()
    }

    /// ペイロードを指定してフレームを作成（デフォルト設定）
    pub fn new(payload: Vec<u8>) -> Result<Self, SerializationError> {
        Self::builder().build(payload)
    }

    /// ヘッダーとペイロードを指定してフレームを作成
    pub fn with_header(
        mut header: UnisonPacketHeader,
        payload: Vec<u8>,
    ) -> Result<Self, SerializationError> {
        let raw_data = PacketSerializer::serialize(&mut header, &payload)?;
        Ok(Self { raw_data })
    }

    /// ヘッダーとペイロードを指定してフレームを作成（カスタム設定）
    pub fn with_header_and_config(
        mut header: UnisonPacketHeader,
        payload: Vec<u8>,
        config: &PacketConfig,
    ) -> Result<Self, SerializationError> {
        let raw_data = PacketSerializer::serialize_with_config(&mut header, &payload, config)?;
        Ok(Self { raw_data })
    }

    /// Bytes からフレームを復元
    pub fn from_bytes(bytes: &Bytes) -> Result<Self, SerializationError> {
        // ヘッダーをパースして互換性をチェック
        let header = PacketDeserializer::parse_header_only(bytes)?;
        if !header.is_compatible() {
            return Err(SerializationError::IncompatibleVersion {
                version: header.version,
            });
        }

        let default_config = PacketConfig::default();
        if bytes.len() > default_config.max_payload_size {
            return Err(SerializationError::PacketTooLarge {
                size: bytes.len(),
                max_size: default_config.max_payload_size,
            });
        }

        Ok(Self {
            raw_data: bytes.clone(),
        })
    }

    /// フレームを Bytes に変換
    pub fn to_bytes(&self) -> Bytes {
        self.raw_data.clone()
    }

    /// 生のバイトデータへの参照を取得
    pub fn as_bytes(&self) -> &[u8] {
        &self.raw_data
    }

    /// フレームサイズを取得
    pub fn size(&self) -> usize {
        self.raw_data.len()
    }

    /// ヘッダーを取得
    pub fn header(&self) -> Result<UnisonPacketHeader, SerializationError> {
        PacketDeserializer::parse_header_only(&self.raw_data)
    }

    /// ペイロードを取得（圧縮されていれば解凍してから返す）
    pub fn payload(&self) -> Result<Vec<u8>, SerializationError> {
        let (_header, payload) = PacketDeserializer::parse(&self.raw_data)?;
        Ok(payload)
    }
}

/// UnisonPacket ビルダー
pub struct UnisonPacketBuilder {
    header: UnisonPacketHeader,
}

impl UnisonPacketBuilder {
    pub fn new() -> Self {
        Self {
            header: UnisonPacketHeader::new(PacketType::Data),
        }
    }

    /// フレームタイプを設定
    pub fn packet_type(mut self, packet_type: PacketType) -> Self {
        self.header.set_packet_type(packet_type);
        self
    }

    /// シーケンス番号を設定
    pub fn with_sequence(mut self, seq: u64) -> Self {
        self.header.sequence_number = seq;
        self
    }

    /// ストリームIDを設定
    pub fn with_stream_id(mut self, id: u64) -> Self {
        self.header.stream_id = id;
        self
    }

    /// メッセージIDを設定（Request/Response識別用）
    pub fn with_message_id(mut self, id: u64) -> Self {
        self.header.message_id = id;
        self
    }

    /// 応答先メッセージIDを設定（Response の場合）
    pub fn with_response_to(mut self, id: u64) -> Self {
        self.header.response_to = id;
        self
    }

    /// 高優先度フラグを設定
    pub fn with_high_priority(mut self) -> Self {
        let mut flags = self.header.flags();
        flags.set(PacketFlags::PRIORITY_HIGH);
        self.header.set_flags(flags);
        self
    }

    /// ACK 要求フラグを設定
    pub fn requires_ack(mut self) -> Self {
        let mut flags = self.header.flags();
        flags.set(PacketFlags::REQUIRES_ACK);
        self.header.set_flags(flags);
        self
    }

    /// カスタムフラグを設定
    pub fn with_flags(mut self, flags: PacketFlags) -> Self {
        self.header.set_flags(flags);
        self
    }

    /// フレームを構築
    pub fn build(mut self, payload: Vec<u8>) -> Result<UnisonPacket, SerializationError> {
        self.header.update_timestamp();
        UnisonPacket::with_header(self.header, payload)
    }
}

impl Default for UnisonPacketBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_creation() {
        let payload = b"Test packet".to_vec();
        let packet = UnisonPacket::new(payload.clone()).unwrap();

        // 最低でも u32 prefix + header + payload より大きい
        assert!(packet.size() > 4 + payload.len());

        let header = packet.header().unwrap();
        assert_eq!(header.packet_type(), PacketType::Data);

        let restored_payload = packet.payload().unwrap();
        assert_eq!(restored_payload, payload);
    }

    #[test]
    fn test_packet_builder() {
        let payload = b"Builder test".to_vec();
        let packet = UnisonPacket::builder()
            .packet_type(PacketType::Control)
            .with_sequence(42)
            .with_stream_id(1337)
            .with_high_priority()
            .build(payload)
            .unwrap();

        let header = packet.header().unwrap();
        assert_eq!(header.packet_type(), PacketType::Control);
        assert_eq!(header.sequence_number, 42);
        assert_eq!(header.stream_id, 1337);
        assert!(header.flags().is_high_priority());
    }

    #[test]
    fn test_round_trip() {
        let original = b"Round trip test".to_vec();
        let packet = UnisonPacket::new(original.clone()).unwrap();

        let bytes = packet.to_bytes();
        let restored_packet = UnisonPacket::from_bytes(&bytes).unwrap();
        let restored = restored_packet.payload().unwrap();

        assert_eq!(original, restored);
    }

    #[test]
    fn test_large_payload_compression() {
        // 圧縮閾値を超える大きなペイロード
        let large_text = "x".repeat(3000);
        let payload = large_text.as_bytes().to_vec();
        let packet = UnisonPacket::new(payload).unwrap();

        let header = packet.header().unwrap();
        assert!(header.is_compressed());
        assert!(header.compressed_length > 0);
        assert!(header.compressed_length < header.payload_length);

        // ラウンドトリップテスト
        let bytes = packet.to_bytes();
        let restored_packet = UnisonPacket::from_bytes(&bytes).unwrap();
        let restored = restored_packet.payload().unwrap();
        assert_eq!(String::from_utf8(restored).unwrap(), large_text);
    }

    #[test]
    fn test_request_response_pattern() {
        // Request 作成
        let request = UnisonPacket::builder()
            .with_message_id(100)
            .with_response_to(0)
            .build(b"Request data".to_vec())
            .unwrap();

        let req_header = request.header().unwrap();
        assert!(req_header.is_request());
        assert_eq!(req_header.message_id, 100);

        // Response 作成（Request の ID を参照）
        let response = UnisonPacket::builder()
            .with_message_id(101)
            .with_response_to(100)
            .build(b"Response data".to_vec())
            .unwrap();

        let res_header = response.header().unwrap();
        assert!(res_header.is_response());
        assert_eq!(res_header.response_to, 100);
    }

    #[test]
    fn test_oneway_message() {
        let oneway = UnisonPacket::builder()
            .with_message_id(0)
            .with_response_to(0)
            .build(b"Oneway message".to_vec())
            .unwrap();

        let header = oneway.header().unwrap();
        assert!(header.is_oneway());
        assert_eq!(header.message_id, 0);
        assert_eq!(header.response_to, 0);
    }
}
