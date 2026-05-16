//! フレームヘッダーの定義
//!
//! UnisonPacket のヘッダー構造。 v0.9.0 buffa pivot で rkyv 固定 56-byte header
//! → buffa-encoded variable-size header に redesign された。 wire 上は
//! length-prefix (= u32 BE) で boundary を明示する形に切り替わっている。
//! 詳細は `spec/02 §8.4` と `design/wire-format.md` を参照。

use uuid::Uuid;

use super::flags::PacketFlags;
use crate::proto;

/// フレームタイプを定義する列挙型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// 通常のデータフレーム
    Data,
    /// 制御メッセージ
    Control,
    /// キープアライブ
    Heartbeat,
    /// ハンドシェイク
    Handshake,
    /// カスタムタイプ（アプリケーション定義）
    Custom(u8),
}

impl From<u8> for PacketType {
    fn from(value: u8) -> Self {
        match value {
            0x00 => Self::Data,
            0x01 => Self::Control,
            0x02 => Self::Heartbeat,
            0x03 => Self::Handshake,
            v => Self::Custom(v),
        }
    }
}

impl From<PacketType> for u8 {
    fn from(pt: PacketType) -> Self {
        match pt {
            PacketType::Data => 0x00,
            PacketType::Control => 0x01,
            PacketType::Heartbeat => 0x02,
            PacketType::Handshake => 0x03,
            PacketType::Custom(v) => v,
        }
    }
}

/// UnisonPacket のヘッダー構造
///
/// buffa (protobuf) でシリアライズされる variable size header。
/// wire 上の boundary は packet 全体の先頭 u32 BE prefix で明示される。
#[derive(Debug, Clone)]
pub struct UnisonPacketHeader {
    /// プロトコルバージョン（現在: 0x01）
    pub version: u8,

    /// フレームタイプ
    pub packet_type: u8,

    /// ビットフラグ（PacketFlags）
    pub flags: u16,

    /// 圧縮前のペイロード長（バイト）
    pub payload_length: u32,

    /// 圧縮後のペイロード長（0=非圧縮）
    pub compressed_length: u32,

    /// シーケンス番号
    pub sequence_number: u64,

    /// タイムスタンプ（Unix timestamp、ナノ秒）
    pub timestamp: u64,

    /// ストリーム識別子
    pub stream_id: u64,

    /// メッセージID（このメッセージの一意な識別子）
    pub message_id: u64,

    /// 応答先メッセージID（0=Request/Oneway, >0=Response）
    pub response_to: u64,

    /// 相関ID（UUID v7）。リクエスト追跡用にクライアントが生成し、
    /// packet flow を通じて伝播する。`None` なら未設定。
    pub correlation_id: Option<Uuid>,
}

impl UnisonPacketHeader {
    /// 現在のプロトコルバージョン
    pub const CURRENT_VERSION: u8 = 0x01;

    /// 新しいヘッダーを作成
    pub fn new(packet_type: PacketType) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            packet_type: packet_type.into(),
            flags: 0,
            payload_length: 0,
            compressed_length: 0,
            sequence_number: 0,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            stream_id: 0,
            message_id: 0,
            response_to: 0,
            correlation_id: None,
        }
    }

    /// フレームタイプを取得
    pub fn packet_type(&self) -> PacketType {
        PacketType::from(self.packet_type)
    }

    /// フレームタイプを設定
    pub fn set_packet_type(&mut self, packet_type: PacketType) {
        self.packet_type = packet_type.into();
    }

    /// フラグを取得
    pub fn flags(&self) -> PacketFlags {
        PacketFlags::from(self.flags)
    }

    /// フラグを設定
    pub fn set_flags(&mut self, flags: PacketFlags) {
        self.flags = flags.into();
    }

    /// 圧縮されているかチェック
    pub fn is_compressed(&self) -> bool {
        self.compressed_length > 0 && self.flags().is_compressed()
    }

    /// バージョンの互換性をチェック
    pub fn is_compatible(&self) -> bool {
        self.version == Self::CURRENT_VERSION
    }

    /// ペイロードの実際のサイズを取得（圧縮されている場合は圧縮後のサイズ）
    pub fn actual_payload_size(&self) -> u32 {
        if self.compressed_length > 0 {
            self.compressed_length
        } else {
            self.payload_length
        }
    }

    /// シーケンス番号を設定
    pub fn with_sequence(mut self, seq: u64) -> Self {
        self.sequence_number = seq;
        self
    }

    /// ストリームIDを設定
    pub fn with_stream_id(mut self, id: u64) -> Self {
        self.stream_id = id;
        self
    }

    /// メッセージIDを設定
    pub fn with_message_id(mut self, id: u64) -> Self {
        self.message_id = id;
        self
    }

    /// 応答先メッセージIDを設定
    pub fn with_response_to(mut self, id: u64) -> Self {
        self.response_to = id;
        self
    }

    /// 相関IDを設定
    pub fn with_correlation_id(mut self, id: Uuid) -> Self {
        self.correlation_id = Some(id);
        self
    }

    /// 新しい相関ID（UUID v7）を生成して設定
    ///
    /// クライアントが request 起点で呼び、以降の packet flow に伝播させる。
    pub fn with_new_correlation_id(mut self) -> Self {
        self.correlation_id = Some(Uuid::now_v7());
        self
    }

    /// このメッセージがRequestかチェック
    pub fn is_request(&self) -> bool {
        self.response_to == 0 && self.message_id != 0
    }

    /// このメッセージがResponseかチェック
    pub fn is_response(&self) -> bool {
        self.response_to != 0
    }

    /// このメッセージがOnewayかチェック
    pub fn is_oneway(&self) -> bool {
        self.response_to == 0 && self.message_id == 0
    }

    /// 現在のタイムスタンプを更新
    pub fn update_timestamp(&mut self) {
        self.timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
    }

    /// 内部 buffa-generated 型へ変換 (serialization 用)
    pub(crate) fn to_proto(&self) -> proto::PacketHeader {
        proto::PacketHeader {
            version: self.version as u32,
            packet_type: self.packet_type as u32,
            flags: self.flags as u32,
            payload_length: self.payload_length,
            compressed_length: self.compressed_length,
            sequence_number: self.sequence_number,
            timestamp: self.timestamp,
            stream_id: self.stream_id,
            message_id: self.message_id,
            response_to: self.response_to,
            // 未設定なら空 byte 列 (= proto3 default)、設定済みなら 16 byte raw
            correlation_id: self
                .correlation_id
                .map(|id| id.as_bytes().to_vec())
                .unwrap_or_default(),
            __buffa_unknown_fields: Default::default(),
        }
    }

    /// buffa-generated 型から復元 (deserialization 用)
    pub(crate) fn from_proto(p: &proto::PacketHeader) -> Self {
        Self {
            version: p.version as u8,
            packet_type: p.packet_type as u8,
            flags: p.flags as u16,
            payload_length: p.payload_length,
            compressed_length: p.compressed_length,
            sequence_number: p.sequence_number,
            timestamp: p.timestamp,
            stream_id: p.stream_id,
            message_id: p.message_id,
            response_to: p.response_to,
            // 16 byte ちょうどのときだけ Uuid として復元 (= 空 / 不正長は None)
            correlation_id: <[u8; 16]>::try_from(p.correlation_id.as_slice())
                .ok()
                .map(Uuid::from_bytes),
        }
    }
}

impl Default for UnisonPacketHeader {
    fn default() -> Self {
        Self::new(PacketType::Data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_creation() {
        let header = UnisonPacketHeader::new(PacketType::Data);
        assert_eq!(header.version, UnisonPacketHeader::CURRENT_VERSION);
        assert_eq!(header.packet_type(), PacketType::Data);
        assert_eq!(header.payload_length, 0);
        assert_eq!(header.compressed_length, 0);
        assert!(!header.is_compressed());
    }

    #[test]
    fn test_packet_type_conversion() {
        assert_eq!(u8::from(PacketType::Data), 0x00);
        assert_eq!(u8::from(PacketType::Control), 0x01);
        assert_eq!(u8::from(PacketType::Heartbeat), 0x02);
        assert_eq!(u8::from(PacketType::Handshake), 0x03);
        assert_eq!(u8::from(PacketType::Custom(0xFF)), 0xFF);

        assert_eq!(PacketType::from(0x00), PacketType::Data);
        assert_eq!(PacketType::from(0x01), PacketType::Control);
        assert_eq!(PacketType::from(0x02), PacketType::Heartbeat);
        assert_eq!(PacketType::from(0x03), PacketType::Handshake);
        assert_eq!(PacketType::from(0xFF), PacketType::Custom(0xFF));
    }

    #[test]
    fn test_flags_integration() {
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        let mut flags = PacketFlags::new();
        flags.set(PacketFlags::COMPRESSED | PacketFlags::PRIORITY_HIGH);

        header.set_flags(flags);
        assert_eq!(header.flags().bits(), flags.bits());
        assert!(header.flags().is_compressed());
        assert!(header.flags().is_high_priority());
    }

    #[test]
    fn test_builder_pattern() {
        let header = UnisonPacketHeader::new(PacketType::Control)
            .with_sequence(42)
            .with_stream_id(1337);

        assert_eq!(header.sequence_number, 42);
        assert_eq!(header.stream_id, 1337);
    }

    #[test]
    fn test_actual_payload_size() {
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        header.payload_length = 1000;
        assert_eq!(header.actual_payload_size(), 1000);

        header.compressed_length = 500;
        let mut flags = PacketFlags::new();
        flags.set(PacketFlags::COMPRESSED);
        header.set_flags(flags);
        assert_eq!(header.actual_payload_size(), 500);
    }

    #[test]
    fn test_message_type_request() {
        let header = UnisonPacketHeader::new(PacketType::Data)
            .with_message_id(123)
            .with_response_to(0);

        assert!(header.is_request());
        assert!(!header.is_response());
        assert!(!header.is_oneway());
    }

    #[test]
    fn test_message_type_response() {
        let header = UnisonPacketHeader::new(PacketType::Data)
            .with_message_id(456)
            .with_response_to(123);

        assert!(!header.is_request());
        assert!(header.is_response());
        assert!(!header.is_oneway());
        assert_eq!(header.response_to, 123);
    }

    #[test]
    fn test_message_type_oneway() {
        let header = UnisonPacketHeader::new(PacketType::Data)
            .with_message_id(0)
            .with_response_to(0);

        assert!(!header.is_request());
        assert!(!header.is_response());
        assert!(header.is_oneway());
    }

    #[test]
    fn test_proto_round_trip() {
        // to_proto / from_proto で fields が完全保存されること
        let mut header = UnisonPacketHeader::new(PacketType::Control)
            .with_sequence(42)
            .with_stream_id(7)
            .with_message_id(1)
            .with_response_to(2)
            .with_new_correlation_id();
        header.payload_length = 128;
        header.compressed_length = 64;
        let mut flags = PacketFlags::new();
        flags.set(PacketFlags::COMPRESSED | PacketFlags::PRIORITY_HIGH);
        header.set_flags(flags);

        let proto = header.to_proto();
        let restored = UnisonPacketHeader::from_proto(&proto);

        assert_eq!(restored.version, header.version);
        assert_eq!(restored.packet_type, header.packet_type);
        assert_eq!(restored.flags, header.flags);
        assert_eq!(restored.payload_length, header.payload_length);
        assert_eq!(restored.compressed_length, header.compressed_length);
        assert_eq!(restored.sequence_number, header.sequence_number);
        assert_eq!(restored.stream_id, header.stream_id);
        assert_eq!(restored.message_id, header.message_id);
        assert_eq!(restored.response_to, header.response_to);
        assert_eq!(restored.correlation_id, header.correlation_id);
    }

    #[test]
    fn test_correlation_id_proto_round_trip() {
        // 未設定 (None) は wire を通っても None のまま
        let header = UnisonPacketHeader::new(PacketType::Data);
        assert_eq!(header.correlation_id, None);
        let restored = UnisonPacketHeader::from_proto(&header.to_proto());
        assert_eq!(restored.correlation_id, None);

        // 設定済み UUID v7 は完全保存される
        let id = uuid::Uuid::now_v7();
        let header = UnisonPacketHeader::new(PacketType::Data).with_correlation_id(id);
        let restored = UnisonPacketHeader::from_proto(&header.to_proto());
        assert_eq!(restored.correlation_id, Some(id));
    }
}
