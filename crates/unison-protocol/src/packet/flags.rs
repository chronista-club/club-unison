//! フレームフラグの定義とビット操作ユーティリティ
//!
//! UnisonPacket で使用されるビットフラグ。 現在 wire に乗るのは `COMPRESSED`
//! (bit 0) のみ。 bit 1-15 は未使用 (= 必要になった時点で用途と一緒に定義する)。

/// フレームフラグを表すビットフィールド
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PacketFlags(pub u16);

impl PacketFlags {
    /// ペイロードが zstd 圧縮されている
    pub const COMPRESSED: u16 = 0b0000_0000_0000_0001; // bit 0

    /// 新しい空のフラグセットを作成
    pub fn new() -> Self {
        Self(0)
    }

    /// 生のビット値を取得
    pub fn bits(&self) -> u16 {
        self.0
    }

    /// フラグを設定
    pub fn set(&mut self, flag: u16) {
        self.0 |= flag;
    }

    /// フラグをクリア
    pub fn unset(&mut self, flag: u16) {
        self.0 &= !flag;
    }

    /// フラグが設定されているかチェック
    pub fn contains(&self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    /// 圧縮フラグが設定されているかチェック
    pub fn is_compressed(&self) -> bool {
        self.contains(Self::COMPRESSED)
    }
}

impl From<u16> for PacketFlags {
    fn from(bits: u16) -> Self {
        Self(bits)
    }
}

impl From<PacketFlags> for u16 {
    fn from(flags: PacketFlags) -> Self {
        flags.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flag_operations() {
        let mut flags = PacketFlags::new();
        assert_eq!(flags.bits(), 0);
        assert!(!flags.is_compressed());

        flags.set(PacketFlags::COMPRESSED);
        assert!(flags.is_compressed());
        assert!(flags.contains(PacketFlags::COMPRESSED));

        flags.unset(PacketFlags::COMPRESSED);
        assert!(!flags.is_compressed());
        assert_eq!(flags.bits(), 0);
    }

    #[test]
    fn test_u16_round_trip() {
        let flags = PacketFlags::from(PacketFlags::COMPRESSED);
        assert!(flags.is_compressed());
        assert_eq!(u16::from(flags), PacketFlags::COMPRESSED);
    }
}
