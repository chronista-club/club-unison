//! # UnisonPacket - 低レベルバイナリフレームフォーマット
//!
//! Unison Protocolで使用される効率的なフレーム表現を提供します。
//!
//! ## 特徴
//!
//! - **ゼロコピーデシリアライゼーション**: rkyvを使用した高速な読み取り
//! - **自動圧縮**: 2KB以上のペイロードを自動的にzstd圧縮
//! - **型安全**: ジェネリクスによる型安全なペイロード
//! - **効率的**: bytes::Bytesとの相互変換サポート
//!
//! ## 使用例
//!
//! ```ignore
//! use club_unison::packet::{UnisonPacket, StringPayload};
//!
//! // フレーム作成
//! let payload = StringPayload::from_string("Hello, World!");
//! let packet = UnisonPacket::builder()
//!     .with_stream_id(123)
//!     .with_sequence(1)
//!     .build(payload)?;
//!
//! // Bytesに変換（ネットワーク送信用）
//! let bytes = packet.to_bytes()?;
//!
//! // Bytesから復元
//! let restored = UnisonPacket::<StringPayload>::from_bytes(&bytes)?;
//! ```

pub mod config;
pub mod flags;
pub mod header;
pub mod payload;
pub mod serialization;

// 主要な型を再エクスポート
pub use config::{CompressionConfig, PacketConfig};
pub use flags::PacketFlags;
pub use header::{PacketType, UnisonPacketHeader};
pub use payload::{
    BytesPayload, EmptyPayload, JsonPayload, PayloadError, Payloadable, RkyvPayload, StringPayload,
};
pub use serialization::{PacketDeserializer, PacketSerializer, SerializationError};

use bytes::Bytes;
use rkyv::Deserialize;
use std::marker::PhantomData;

/// UnisonPacket - ジェネリックなペイロードを持つフレーム
///
/// 実際のフレームデータはBytesとして保持され、
/// 必要に応じてペイロードをデシリアライズします。
pub struct UnisonPacket<T>
where
    T: Payloadable,
{
    /// シリアライズされたフレームデータ
    raw_data: Bytes,
    /// ペイロード型のマーカー
    _phantom: PhantomData<T>,
}

impl<T> UnisonPacket<T>
where
    T: Payloadable,
{
    /// フレームビルダーを作成
    pub fn builder() -> UnisonPacketBuilder<T> {
        UnisonPacketBuilder::new()
    }

    /// 指定したペイロードでフレームを作成
    pub fn new(payload: T) -> Result<Self, SerializationError> {
        Self::builder().build(payload)
    }

    /// ヘッダーとペイロードを指定してフレームを作成
    pub fn with_header(
        mut header: UnisonPacketHeader,
        payload: T,
    ) -> Result<Self, SerializationError> {
        let raw_data = PacketSerializer::serialize(&mut header, &payload)?;
        Ok(Self {
            raw_data,
            _phantom: PhantomData,
        })
    }

    /// ヘッダーとペイロードを指定してフレームを作成（カスタム設定）
    pub fn with_header_and_config(
        mut header: UnisonPacketHeader,
        payload: T,
        config: &PacketConfig,
    ) -> Result<Self, SerializationError> {
        let raw_data = PacketSerializer::serialize_with_config(&mut header, &payload, config)?;
        Ok(Self {
            raw_data,
            _phantom: PhantomData,
        })
    }

    /// Bytesからフレームを復元
    pub fn from_bytes(bytes: &Bytes) -> Result<Self, SerializationError> {
        // ヘッダーの検証のみ行う（ペイロードは遅延デシリアライズ）
        let (header, _) = PacketDeserializer::deserialize_header(bytes)?;

        // バージョンとサイズのチェック
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
            _phantom: PhantomData,
        })
    }

    /// フレームをBytesに変換
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
        let (header, _) = PacketDeserializer::deserialize_header(&self.raw_data)?;
        Ok(header)
    }

    /// ペイロードを取得（デシリアライズ）
    pub fn payload(&self) -> Result<T, SerializationError>
    where
        T::Archived: Deserialize<T, rkyv::Infallible>,
        for<'a> T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'a>>,
    {
        let (header, payload_bytes) = PacketDeserializer::deserialize_header(&self.raw_data)?;
        PacketDeserializer::deserialize_payload(&header, &payload_bytes)
    }

    /// ペイロードをゼロコピーで参照（アーカイブされた形式）
    pub fn payload_zero_copy<'a>(
        &'a self,
        buffer: &'a mut Vec<u8>,
    ) -> Result<&'a T::Archived, SerializationError>
    where
        for<'b> T::Archived: rkyv::CheckBytes<rkyv::validation::validators::DefaultValidator<'b>>,
    {
        let (header, _) = PacketDeserializer::deserialize_header(&self.raw_data)?;

        // ヘッダーサイズをスキップしてペイロード部分を取得
        let payload_start = UnisonPacketHeader::SERIALIZED_SIZE;
        let payload_bytes = &self.raw_data[payload_start..];

        PacketDeserializer::deserialize_payload_zero_copy::<T>(&header, payload_bytes, buffer)
    }
}

/// UnisonPacketビルダー
///
/// フレームの各種パラメータを設定してフレームを構築します。
pub struct UnisonPacketBuilder<T>
where
    T: Payloadable,
{
    header: UnisonPacketHeader,
    _phantom: PhantomData<T>,
}

impl<T> UnisonPacketBuilder<T>
where
    T: Payloadable,
{
    /// 新しいビルダーを作成
    pub fn new() -> Self {
        Self {
            header: UnisonPacketHeader::new(PacketType::Data),
            _phantom: PhantomData,
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

    /// 応答先メッセージIDを設定（Responseの場合）
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

    /// ACK要求フラグを設定
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
    pub fn build(mut self, payload: T) -> Result<UnisonPacket<T>, SerializationError> {
        // タイムスタンプを更新
        self.header.update_timestamp();

        UnisonPacket::with_header(self.header, payload)
    }
}

impl<T> Default for UnisonPacketBuilder<T>
where
    T: Payloadable,
{
    fn default() -> Self {
        Self::new()
    }
}

/// UnisonPacketビュー - ゼロコピー読み取り用
///
/// フレームデータを所有せず、参照として保持します。
pub struct UnisonPacketView<'a> {
    header: UnisonPacketHeader,
    payload_bytes: &'a [u8],
    is_compressed: bool,
}

impl<'a> UnisonPacketView<'a> {
    /// Bytesからビューを作成
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self, SerializationError> {
        let header_size = UnisonPacketHeader::SERIALIZED_SIZE;
        if bytes.len() < header_size {
            return Err(SerializationError::InvalidHeader);
        }

        // ヘッダーをパース
        let header_bytes = &bytes[..header_size];
        let archived_header = rkyv::check_archived_root::<UnisonPacketHeader>(header_bytes)
            .map_err(|e| SerializationError::DeserializationFailed(e.to_string()))?;
        let header: UnisonPacketHeader = archived_header
            .deserialize(&mut rkyv::Infallible)
            .map_err(|_| SerializationError::InvalidHeader)?;

        // ペイロード部分を取得
        let payload_bytes = &bytes[header_size..];
        let is_compressed = header.is_compressed();

        Ok(Self {
            header,
            payload_bytes,
            is_compressed,
        })
    }

    /// ヘッダーへの参照を取得
    pub fn header(&self) -> &UnisonPacketHeader {
        &self.header
    }

    /// 圧縮されているかチェック
    pub fn is_compressed(&self) -> bool {
        self.is_compressed
    }

    /// ペイロードサイズを取得
    pub fn payload_size(&self) -> usize {
        self.payload_bytes.len()
    }

    /// 元のペイロードサイズを取得（圧縮前）
    pub fn original_payload_size(&self) -> u32 {
        self.header.payload_length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packet_creation() {
        let payload = StringPayload::from_string("Test packet");
        let packet = UnisonPacket::new(payload.clone()).unwrap();

        assert!(packet.size() > UnisonPacketHeader::SERIALIZED_SIZE);

        let header = packet.header().unwrap();
        assert_eq!(header.packet_type(), PacketType::Data);

        let restored_payload = packet.payload().unwrap();
        assert_eq!(restored_payload.data, payload.data);
    }

    #[test]
    fn test_packet_builder() {
        let payload = StringPayload::from_string("Builder test");
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
        let original = StringPayload::from_string("Round trip test");
        let packet = UnisonPacket::new(original.clone()).unwrap();

        let bytes = packet.to_bytes();
        let restored_packet = UnisonPacket::<StringPayload>::from_bytes(&bytes).unwrap();
        let restored = restored_packet.payload().unwrap();

        assert_eq!(original.data, restored.data);
    }

    #[test]
    fn test_zero_copy_view() {
        let payload = BytesPayload::new(vec![1, 2, 3, 4, 5]);
        let packet = UnisonPacket::new(payload).unwrap();
        let bytes = packet.to_bytes();

        let view = UnisonPacketView::from_bytes(&bytes).unwrap();
        assert_eq!(view.header().packet_type(), PacketType::Data);
        assert!(view.payload_size() > 0);

        let mut buffer = Vec::new();
        let archived = packet.payload_zero_copy(&mut buffer).unwrap();
        assert_eq!(archived.data.as_slice(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_large_payload_compression() {
        // 圧縮閾値を超える大きなペイロード
        let large_text = "x".repeat(3000);
        let payload = StringPayload::new(large_text.clone());
        let packet = UnisonPacket::new(payload).unwrap();

        let header = packet.header().unwrap();
        assert!(header.is_compressed());
        assert!(header.compressed_length > 0);
        assert!(header.compressed_length < header.payload_length);

        // ラウンドトリップテスト
        let bytes = packet.to_bytes();
        let restored_packet = UnisonPacket::<StringPayload>::from_bytes(&bytes).unwrap();
        let restored = restored_packet.payload().unwrap();
        assert_eq!(restored.data, large_text);
    }

    #[test]
    fn test_request_response_pattern() {
        // Request作成
        let request_payload = StringPayload::from_string("Request data");
        let request = UnisonPacket::builder()
            .with_message_id(100)
            .with_response_to(0)
            .build(request_payload)
            .unwrap();

        let req_header = request.header().unwrap();
        assert!(req_header.is_request());
        assert_eq!(req_header.message_id, 100);

        // Response作成（RequestのIDを参照）
        let response_payload = StringPayload::from_string("Response data");
        let response = UnisonPacket::builder()
            .with_message_id(101)
            .with_response_to(100) // Requestのmessage_id
            .build(response_payload)
            .unwrap();

        let res_header = response.header().unwrap();
        assert!(res_header.is_response());
        assert_eq!(res_header.response_to, 100);
    }

    #[test]
    fn test_oneway_message() {
        let payload = StringPayload::from_string("Oneway message");
        let oneway = UnisonPacket::builder()
            .with_message_id(0)
            .with_response_to(0)
            .build(payload)
            .unwrap();

        let header = oneway.header().unwrap();
        assert!(header.is_oneway());
        assert_eq!(header.message_id, 0);
        assert_eq!(header.response_to, 0);
    }
}
