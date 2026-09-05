//! フレームヘッダーの定義
//!
//! UnisonPacket のヘッダー構造。 v0.9.0 buffa pivot で rkyv 固定 56-byte header
//! → buffa-encoded variable-size header に redesign された。 wire 上は
//! length-prefix (= u32 BE) で boundary を明示する形に切り替わっている。
//! 詳細は `spec/02 §8.4` と `design/wire-format.md` を参照。

use super::flags::PacketFlags;
use crate::proto;

/// フレームタイプを定義する列挙型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    /// 通常のデータフレーム
    Data,
    /// 制御メッセージ
    Control,
}

impl TryFrom<u8> for PacketType {
    /// 未知の値はそのまま返す (= caller が log / reject を判断できる)
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, u8> {
        match value {
            0x00 => Ok(Self::Data),
            0x01 => Ok(Self::Control),
            v => Err(v),
        }
    }
}

impl From<PacketType> for u8 {
    fn from(pt: PacketType) -> Self {
        match pt {
            PacketType::Data => 0x00,
            PacketType::Control => 0x01,
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

    /// タイムスタンプ（Unix timestamp、ナノ秒）
    pub timestamp: u64,
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
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
        }
    }

    /// フレームタイプを取得 (= 未知の値は `Err(raw)`)
    pub fn packet_type(&self) -> Result<PacketType, u8> {
        PacketType::try_from(self.packet_type)
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

    /// 内部 buffa-generated 型へ変換 (serialization 用)
    pub(crate) fn to_proto(&self) -> proto::PacketHeader {
        proto::PacketHeader {
            version: self.version as u32,
            packet_type: self.packet_type as u32,
            flags: self.flags as u32,
            payload_length: self.payload_length,
            compressed_length: self.compressed_length,
            timestamp: self.timestamp,
            __buffa_unknown_fields: Default::default(),
        }
    }

    /// buffa-generated 型から復元 (deserialization 用)
    ///
    /// proto 上は version/packet_type/flags が u32。 `as u8`/`as u16` の素朴な cast は
    /// silently truncate するため、 例えば version=257 が 1 に化けて
    /// [`Self::is_compatible`] の gate を素通りしてしまう。 範囲外は飽和 (u8::MAX /
    /// u16::MAX) させ、 「正規の version/type を詐称できない」 = 確実に incompatible /
    /// unknown と判定されるようにする (= untrusted wire 入力への堅牢化)。
    pub(crate) fn from_proto(p: &proto::PacketHeader) -> Self {
        Self {
            version: u8::try_from(p.version).unwrap_or(u8::MAX),
            packet_type: u8::try_from(p.packet_type).unwrap_or(u8::MAX),
            flags: u16::try_from(p.flags).unwrap_or(u16::MAX),
            payload_length: p.payload_length,
            compressed_length: p.compressed_length,
            timestamp: p.timestamp,
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
        assert_eq!(header.packet_type(), Ok(PacketType::Data));
        assert_eq!(header.payload_length, 0);
        assert_eq!(header.compressed_length, 0);
        assert!(!header.is_compressed());
    }

    #[test]
    fn test_packet_type_conversion() {
        assert_eq!(u8::from(PacketType::Data), 0x00);
        assert_eq!(u8::from(PacketType::Control), 0x01);

        assert_eq!(PacketType::try_from(0x00), Ok(PacketType::Data));
        assert_eq!(PacketType::try_from(0x01), Ok(PacketType::Control));
        assert_eq!(PacketType::try_from(0xFF), Err(0xFF));
    }

    #[test]
    fn test_flags_integration() {
        let mut header = UnisonPacketHeader::new(PacketType::Data);
        let mut flags = PacketFlags::new();
        flags.set(PacketFlags::COMPRESSED);

        header.set_flags(flags);
        assert_eq!(header.flags().bits(), flags.bits());
        assert!(header.flags().is_compressed());
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
    fn test_proto_round_trip() {
        // to_proto / from_proto で fields が完全保存されること
        let mut header = UnisonPacketHeader::new(PacketType::Control);
        header.payload_length = 128;
        header.compressed_length = 64;
        let mut flags = PacketFlags::new();
        flags.set(PacketFlags::COMPRESSED);
        header.set_flags(flags);

        let proto = header.to_proto();
        let restored = UnisonPacketHeader::from_proto(&proto);

        assert_eq!(restored.version, header.version);
        assert_eq!(restored.packet_type, header.packet_type);
        assert_eq!(restored.flags, header.flags);
        assert_eq!(restored.payload_length, header.payload_length);
        assert_eq!(restored.compressed_length, header.compressed_length);
        assert_eq!(restored.timestamp, header.timestamp);
    }

    #[test]
    fn from_proto_saturates_out_of_range_fields() {
        // proto は u32。 u8/u16 範囲外は truncate せず飽和させ、 正規値を詐称させない。
        let mut p = UnisonPacketHeader::new(PacketType::Data).to_proto();
        p.version = 257; // = 0x101、 truncate すると 1 (= CURRENT_VERSION) に化ける
        p.packet_type = 300;
        p.flags = 0x1_0000; // u16 範囲外

        let h = UnisonPacketHeader::from_proto(&p);

        assert_eq!(
            h.version,
            u8::MAX,
            "範囲外 version は飽和すべき (truncate 不可)"
        );
        assert!(
            !h.is_compatible(),
            "範囲外 version は compat gate を通ってはならない"
        );
        assert_eq!(h.packet_type, u8::MAX);
        assert_eq!(h.packet_type(), Err(u8::MAX));
        assert_eq!(h.flags, u16::MAX);
    }
}
