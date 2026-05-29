//! Typed-frame の wire I/O。
//!
//! Length-prefixed フレーム ([`read_frame`] / [`write_frame`]) と type tag 付き
//! typed フレーム ([`read_typed_frame`] / [`write_typed_frame`]) の読み書きを
//! 提供する。 `quinn::RecvStream` / `quinn::SendStream` のみならず WebTransport
//! のストリームでも使えるよう、 `AsyncRead` / `AsyncWrite` でジェネリック化されて
//! いる (= transport 非依存)。

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::ProtocolMessage;

/// Maximum message size for QUIC streams (8MB)
const MAX_MESSAGE_SIZE: usize = 8 * 1024 * 1024;

/// Length-prefixed フレームの読み取り（4バイトBE長 + データ）
/// ストリームを消費せずに1フレームだけ読む
///
/// `quinn::RecvStream` のみならず WebTransport のストリームでも使えるよう、
/// `AsyncRead` でジェネリック化されている (= transport 非依存)。
pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(recv: &mut R) -> Result<bytes::Bytes> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .context("Failed to read frame length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE {
        return Err(anyhow::anyhow!("Frame too large: {} bytes", len));
    }
    let mut data = vec![0u8; len];
    recv.read_exact(&mut data)
        .await
        .context("Failed to read frame data")?;
    Ok(bytes::Bytes::from(data))
}

/// Length-prefixed フレームの書き込み
pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(send: &mut W, data: &[u8]) -> Result<()> {
    // read 側と対称な上限。 これが無いと data.len() >= 4GiB で `as u32` が silently
    // truncate し、 wire 上の length prefix が実データ長と食い違って frame が壊れる。
    if data.len() > MAX_MESSAGE_SIZE {
        return Err(anyhow::anyhow!(
            "Frame too large to write: {} bytes (max {})",
            data.len(),
            MAX_MESSAGE_SIZE
        ));
    }
    let len = (data.len() as u32).to_be_bytes();
    send.write_all(&len)
        .await
        .context("Failed to write frame length")?;
    send.write_all(data)
        .await
        .context("Failed to write frame data")?;
    Ok(())
}

/// フレームタイプタグ
pub const FRAME_TYPE_PROTOCOL: u8 = 0x00;
pub const FRAME_TYPE_RAW: u8 = 0x01;

/// Channel open ack の method 名 (= Phase 6c)。
///
/// クライアントが `__channel:{name}` open frame を送ると、 サーバーは登録済み
/// channel を lookup し、 同 stream へこの method の `ProtocolMessage` を 1 本返す:
/// - **accept**: `msg_type = Response`、 `id` = open request の id、 payload `{}`
/// - **nack** (= channel-not-found): `msg_type = Error`、 同 `id`、
///   payload `{"error":"channel-not-found","channel":"{name}"}`
///
/// `id` が open request と一致するため、 クライアントは自分の open request に
/// 相関させられる。 `__identity` と同じ `__`-prefix の特殊 method であり、 新しい
/// typed frame type は追加しない (= 既存 wire layout は不変、 additive)。
pub const CHANNEL_ACK_METHOD: &str = "__channel_ack";

/// Typed フレーム — type tag 付きの読み書き
/// フォーマット: [4 bytes: length][1 byte: type tag][payload]
/// length は type tag + payload の合計バイト数
///
/// Typed フレームの読み取り — type tag とペイロードを返す
pub async fn read_typed_frame<R: AsyncRead + Unpin + ?Sized>(
    recv: &mut R,
) -> Result<(u8, bytes::Bytes)> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .context("Failed to read frame length")?;
    let total_len = u32::from_be_bytes(len_buf) as usize;
    if total_len == 0 {
        return Err(anyhow::anyhow!("Empty frame"));
    }
    if total_len > MAX_MESSAGE_SIZE {
        return Err(anyhow::anyhow!("Frame too large: {} bytes", total_len));
    }

    // type tag を読む
    let mut type_buf = [0u8; 1];
    recv.read_exact(&mut type_buf)
        .await
        .context("Failed to read frame type tag")?;
    let frame_type = type_buf[0];

    // payload を読む
    let payload_len = total_len - 1;
    let mut data = vec![0u8; payload_len];
    recv.read_exact(&mut data)
        .await
        .context("Failed to read frame payload")?;
    Ok((frame_type, bytes::Bytes::from(data)))
}

/// Typed フレームの書き込み
pub async fn write_typed_frame<W: AsyncWrite + Unpin + ?Sized>(
    send: &mut W,
    frame_type: u8,
    data: &[u8],
) -> Result<()> {
    let total_len = (1 + data.len()) as u32;
    send.write_all(&total_len.to_be_bytes())
        .await
        .context("Failed to write frame length")?;
    send.write_all(&[frame_type])
        .await
        .context("Failed to write frame type tag")?;
    send.write_all(data)
        .await
        .context("Failed to write frame payload")?;
    Ok(())
}

/// Channel open ack / nack を 1 本の typed protocol frame として送信する (= Phase 6c)。
///
/// `accepted == true` なら [`MessageType::Response`] の `open_ack`、 `false` なら
/// [`MessageType::Error`] の nack (= payload に `channel-not-found`) を `send`
/// ストリームへ書き出す。 `request_id` は open request の id を引き継ぎ、
/// クライアントが自分の open と相関できるようにする。
///
/// [`MessageType::Response`]: super::MessageType::Response
/// [`MessageType::Error`]: super::MessageType::Error
pub(crate) async fn write_channel_ack<W: AsyncWrite + Unpin + ?Sized>(
    send: &mut W,
    request_id: u64,
    accepted: bool,
    channel_name: &str,
) -> Result<()> {
    use super::MessageType;

    let (msg_type, payload) = if accepted {
        (MessageType::Response, serde_json::json!({}))
    } else {
        (
            MessageType::Error,
            serde_json::json!({
                "error": "channel-not-found",
                "channel": channel_name,
            }),
        )
    };
    let msg = ProtocolMessage::new_with_json(
        request_id,
        CHANNEL_ACK_METHOD.to_string(),
        msg_type,
        payload,
    )
    .map_err(|e| anyhow::anyhow!("Failed to build open_ack message: {}", e))?;
    let frame = msg
        .into_frame()
        .map_err(|e| anyhow::anyhow!("Failed to encode open_ack frame: {}", e))?;
    write_typed_frame(send, FRAME_TYPE_PROTOCOL, &frame.to_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_frame_rejects_oversized() {
        // read 側の上限と対称: MAX_MESSAGE_SIZE 超は書き込み拒否し、 何も書かない
        let mut sink: Vec<u8> = Vec::new();
        let too_big = vec![0u8; MAX_MESSAGE_SIZE + 1];
        let res = write_frame(&mut sink, &too_big).await;
        assert!(res.is_err(), "8MB 超の frame は書き込み拒否されるべき");
        assert!(sink.is_empty(), "拒否時は length prefix すら書かないべき");
    }

    #[tokio::test]
    async fn write_then_read_frame_round_trip() {
        let payload = b"hello frame";
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, payload).await.unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let got = read_frame(&mut cursor).await.unwrap();
        assert_eq!(&got[..], payload);
    }
}
