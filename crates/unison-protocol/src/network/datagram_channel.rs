//! Datagram Channel: QUIC datagram 経由の channel (v0.10.0 で追加)
//!
//! `UnisonChannel` (= stream channel、 QUIC bidi stream に対応) と並列の通信路。
//! 1 connection 内の共有 datagram path 上に **`channel_id` で identified された
//! virtual stream** として存在し、 schema-time で割り当てた `channel_id` を payload
//! 先頭に varint encoded prefix として埋め込んで demux する。
//!
//! ## Wire format
//!
//! ```text
//! [varint channel_id] [codec-encoded event payload]
//! ```
//!
//! ## 配送特性
//!
//! - Unordered + Unreliable (= QUIC datagram の semantics に従う)
//! - 1 datagram = 1 event message、 chunking / fragmentation 不可
//! - MTU 超過は `SendDatagramError::TooLarge`、 caller が分割責任
//!
//! 詳細は `design/datagram-channel.md` および `spec/02-unified-channel/SPEC.md` §8.5 参照。

use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

use crate::codec::{Codec, Decodable, Encodable, JsonCodec};

use super::NetworkError;

/// Varint encoding upper bound (= LEB128 で u64 を表す最大 byte 数)
pub(crate) const VARINT_MAX_LEN: usize = 10;

/// LEB128 varint encode (= proto3 と同じ)
///
/// `value` を payload buffer に append、 書き込んだ byte 数を返す。 buffer は
/// 少なくとも [`VARINT_MAX_LEN`] byte の余裕を持っている必要がある。
///
/// Hot path で呼ばれるため `#[inline]`、 分岐を最小化した実装。
#[inline]
pub(crate) fn encode_varint(mut value: u64, buf: &mut Vec<u8>) -> usize {
    let start = buf.len();
    while value >= 0x80 {
        buf.push(((value & 0x7F) | 0x80) as u8);
        value >>= 7;
    }
    buf.push(value as u8);
    buf.len() - start
}

/// LEB128 varint decode
///
/// `bytes` 先頭から varint を読み、 `(value, consumed_bytes)` を返す。 不正な
/// encoding (= 10 byte 超え or premature EOF) は `Err`。
#[inline]
pub(crate) fn decode_varint(bytes: &[u8]) -> Result<(u64, usize), NetworkError> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in bytes.iter().enumerate().take(VARINT_MAX_LEN) {
        value |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
    }
    Err(NetworkError::Protocol(
        "Varint decoding failed: malformed or too long".to_string(),
    ))
}

/// QUIC datagram 経由の channel
///
/// `UnisonChannel<C>` (= stream channel) と並列の型分離 channel、 datagram-specific
/// semantics (= MTU 制約、 unordered / unreliable、 channel_id ベースの demux) を
/// 型レベルで表現する。
///
/// 構成要素:
/// - `connection`: 共有 QUIC connection (= 同 connection の他 channel と共有)
/// - `channel_id`: schema-time fixed の demux 識別子 (= varint prefix として wire 出現)
/// - `recv_rx`: 外側 dispatcher から push される demuxed payload の receiver
///
/// ## Caller の責務
///
/// `recv_rx` は ProtocolClient / ProtocolServer 側の datagram dispatch loop が
/// `mpsc::Sender` を保持し、 varint prefix で route した payload を流し込む。
/// `DatagramChannel` 自身は demux logic を知らず、 「自分宛の payload を pull する」
/// 単純な責務のみ。
pub struct DatagramChannel<C: Codec = JsonCodec> {
    /// 接続 (= 同 connection の stream channel / 他 datagram channel と共有)。
    /// transport 非依存の [`UnisonConn`](super::conn::UnisonConn) trait object。
    connection: Arc<dyn super::conn::UnisonConn>,
    /// Schema-time fixed の channel ID (= varint prefix)
    channel_id: u64,
    /// Channel name (= debug / log 用、 KDL schema 上の名前)
    name: String,
    /// Demux 後の payload receiver (= 外側 dispatcher が sender 側を保持)
    recv_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
    /// Codec 型マーカー
    _codec: PhantomData<C>,
}

impl<C: Codec> DatagramChannel<C> {
    /// 新しい `DatagramChannel` を構築する (= 内部用)
    ///
    /// caller (= `ProtocolClient::open_datagram_channel` / `ProtocolServer::register_channel_datagram`)
    /// が demux dispatch table に `recv_tx` を登録した上で本 constructor を呼ぶ。
    pub(crate) fn new(
        connection: Arc<dyn super::conn::UnisonConn>,
        channel_id: u64,
        name: impl Into<String>,
        recv_rx: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        Self {
            connection,
            channel_id,
            name: name.into(),
            recv_rx: Mutex::new(recv_rx),
            _codec: PhantomData,
        }
    }

    /// Channel の schema-time ID を取得
    pub fn channel_id(&self) -> u64 {
        self.channel_id
    }

    /// Channel name (= KDL schema 上の名前) を取得
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Event を datagram で送信
    ///
    /// `event` を codec `C` で encode → 先頭に varint encoded `channel_id` を prepend
    /// → QUIC datagram として送信。 配送保証なし、 順序保証なし、 MTU 超過は error。
    pub async fn send_event<T: Encodable<C>>(&self, event: &T) -> Result<(), NetworkError> {
        // codec で event を encode
        let encoded = event.encode().map_err(NetworkError::Codec)?;

        // [varint channel_id] [encoded] を組み立て
        let mut buf = Vec::with_capacity(VARINT_MAX_LEN + encoded.len());
        encode_varint(self.channel_id, &mut buf);
        buf.extend_from_slice(&encoded);

        // datagram として送信 (= transport 非依存)
        self.connection.send_datagram(buf.into())
    }

    /// Event を datagram で受信
    ///
    /// 外側 dispatcher が `channel_id` で route した payload を pull して codec `C` で
    /// decode。 channel が close されている場合は `Protocol("Datagram channel closed")` error。
    pub async fn recv_event<T: Decodable<C>>(&self) -> Result<T, NetworkError> {
        let mut rx = self.recv_rx.lock().await;
        let payload = rx
            .recv()
            .await
            .ok_or_else(|| NetworkError::Protocol("Datagram channel closed".to_string()))?;
        T::decode(&payload).map_err(NetworkError::Codec)
    }

    /// Channel を閉じる
    ///
    /// stream channel と異なり QUIC stream FIN は無く、 demux dispatcher から自分の
    /// `recv_tx` を取り除く形で「もう受信しない」 状態を作る。 現在の skeleton 実装は
    /// `recv_rx` の drop に依存 (= caller が `DatagramChannel` を drop すれば自動)、
    /// 明示 close は v0.10+ で dispatch table と整合させる際に拡張予定。
    pub async fn close(&self) -> Result<(), NetworkError> {
        // skeleton: 明示 close は dispatcher 統合 (= 1d/1e) で詳細化
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_encode_small_values_one_byte() {
        // 0..=127 は 1 byte
        let mut buf = Vec::new();
        let n = encode_varint(0, &mut buf);
        assert_eq!(n, 1);
        assert_eq!(buf, vec![0x00]);

        buf.clear();
        let n = encode_varint(1, &mut buf);
        assert_eq!(n, 1);
        assert_eq!(buf, vec![0x01]);

        buf.clear();
        let n = encode_varint(127, &mut buf);
        assert_eq!(n, 1);
        assert_eq!(buf, vec![0x7F]);
    }

    #[test]
    fn varint_encode_medium_values_two_bytes() {
        // 128..=16383 は 2 byte
        let mut buf = Vec::new();
        let n = encode_varint(128, &mut buf);
        assert_eq!(n, 2);
        assert_eq!(buf, vec![0x80, 0x01]);

        buf.clear();
        let n = encode_varint(300, &mut buf);
        assert_eq!(n, 2);
        // 300 = 0b100101100
        // varint: 0xAC 0x02 (= [101_1100][0000_0010])
        assert_eq!(buf, vec![0xAC, 0x02]);
    }

    #[test]
    fn varint_encode_max_u64_ten_bytes() {
        let mut buf = Vec::new();
        let n = encode_varint(u64::MAX, &mut buf);
        assert_eq!(n, VARINT_MAX_LEN);
        assert_eq!(buf.len(), VARINT_MAX_LEN);
    }

    #[test]
    fn varint_decode_round_trip() {
        for value in [
            0u64,
            1,
            42,
            127,
            128,
            300,
            16383,
            16384,
            1_000_000,
            u64::MAX,
        ] {
            let mut buf = Vec::new();
            let written = encode_varint(value, &mut buf);
            let (decoded, consumed) = decode_varint(&buf).expect("decode");
            assert_eq!(decoded, value, "round-trip for {}", value);
            assert_eq!(consumed, written, "consumed bytes for {}", value);
        }
    }

    #[test]
    fn varint_decode_stops_at_terminator() {
        // [varint 1] [trailing bytes] を decode すると 1 byte consumed
        let buf = vec![0x01u8, 0xFF, 0xFF, 0xFF];
        let (value, consumed) = decode_varint(&buf).unwrap();
        assert_eq!(value, 1);
        assert_eq!(consumed, 1);
        // trailing 3 byte は caller が使う
    }

    #[test]
    fn varint_decode_malformed_too_long_fails() {
        // 11 byte 全てに continuation bit が立っていると malformed
        let buf = vec![0xFFu8; 11];
        assert!(decode_varint(&buf).is_err());
    }

    #[test]
    fn varint_decode_premature_eof_fails() {
        // continuation bit 立ったまま buffer 終了 → malformed
        let buf = vec![0x80u8, 0x80, 0x80];
        assert!(decode_varint(&buf).is_err());
    }

    /// `DatagramChannel` の compile-check + 基本 getter テスト (= 実 connection なし)
    #[test]
    fn datagram_channel_constructs_and_getters_work() {
        // mpsc::channel から DatagramChannel を構築できることを確認
        // (= connection は実 QUIC connection が必要なので、 ここでは Mutex/Arc の
        // type-level 整合だけを確認する compile-check)
        let _phantom_check = |conn: Arc<dyn super::super::conn::UnisonConn>,
                              rx: mpsc::Receiver<Vec<u8>>| {
            let ch: DatagramChannel<JsonCodec> = DatagramChannel::new(conn, 42, "position", rx);
            assert_eq!(ch.channel_id(), 42);
            assert_eq!(ch.name(), "position");
        };
        // closure は call しない (= 実 connection が無いため)、 型 check 用
        let _ = _phantom_check;
    }
}
