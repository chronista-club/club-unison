//! Transport abstraction layer — [`UnisonConn`].
//!
//! # なぜ抽象化するか
//!
//! Unison は元々 raw QUIC (= `quinn`) 専用だった。 v1.0 はブラウザファースト
//! であり、 ブラウザは raw QUIC へ到達できず WebTransport (= QUIC + HTTP/3) を
//! 経由する。 サーバーは raw QUIC と WebTransport の **2 つの ingress** を持つ
//! 必要がある。
//!
//! [`UnisonConn`] は接続を「双方向ストリームの開閉 + datagram + メタ情報」へ
//! 抽象化した trait。 `quic.rs::handle_connection` はこの trait object
//! (`Arc<dyn UnisonConn>`) のみに依存し、 transport の種類を知らない。 これに
//! より genericity が `server.rs` へ波及せず、 抽象化境界は `handle_connection`
//! の入口に閉じ込められる。
//!
//! # ストリーム
//!
//! `quinn` の `SendStream` / `RecvStream` は `tokio` の `AsyncWrite` /
//! `AsyncRead` を実装している。 そのため双方向ストリームは
//! [`BoxUnisonSend`] / [`BoxUnisonRecv`] (= trait object) として扱え、 フレーム
//! I/O (`read_frame` 等) は `impl AsyncRead` / `impl AsyncWrite` でジェネリック
//! 化できる。

use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};

use super::NetworkError;

/// 送信ストリーム — `AsyncWrite` + FIN 送出。
///
/// `quinn::SendStream` / `wtransport::SendStream` の `finish()` 相当を抽象化する。
///
/// `finish` を async にしている理由: `quinn` の `finish` は同期だが
/// `wtransport` の `finish` は async (= HTTP/3 capsule の flush を伴う)。 両方を
/// 同一 trait で扱うため async に統一する。
pub trait UnisonSend: AsyncWrite + Send + Unpin {
    /// ストリームに FIN を送出して正常終了させる。
    ///
    /// QUIC ではピアに「これ以上データは来ない」を通知する。 冪等であること
    /// を期待する (= 二重 `finish` はエラーにしない)。
    fn finish(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), NetworkError>> + Send + '_>>;
}

/// 受信ストリーム — `AsyncRead` + STOP 送出。
///
/// `quinn::RecvStream` / `wtransport::RecvStream` の `stop()` 相当を抽象化する。
pub trait UnisonRecv: AsyncRead + Send + Unpin {
    /// ストリームの受信側を停止し、 ピアへ STOP_SENDING を送出する。
    fn stop(&mut self) -> Result<(), NetworkError>;
}

/// ボックス化した送信ストリーム (= trait object)。
pub type BoxUnisonSend = Box<dyn UnisonSend>;

/// ボックス化した受信ストリーム (= trait object)。
pub type BoxUnisonRecv = Box<dyn UnisonRecv>;

/// 双方向ストリームのペア (= `open_bi` / `accept_bi` の結果)。
pub type BiStream = (BoxUnisonSend, BoxUnisonRecv);

/// transport-agnostic な接続抽象。
///
/// raw QUIC (`quinn::Connection`) と WebTransport セッションの両方がこの trait
/// を実装し、 `handle_connection` は `Arc<dyn UnisonConn>` のみに依存する。
///
/// すべてのメソッドは `Box<dyn Future>` を返す (= `async fn` in trait の dyn-safe
/// 版)。 trait object として使うため async fn を直接置けない。
pub trait UnisonConn: Send + Sync {
    /// ピアが開いた双方向ストリームを受け付ける。
    ///
    /// 接続がクローズ済みの場合は `Err` を返す (= caller はループを終える)。
    fn accept_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>;

    /// 双方向ストリームを開く。
    fn open_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>;

    /// datagram を送信する (= 信頼性なし / 順序なし、 ≤MTU)。
    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), NetworkError>;

    /// 次の datagram を受信する (= 到着 or 接続クローズまでブロック)。
    fn recv_datagram(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bytes::Bytes, NetworkError>> + Send + '_>>;

    /// リモートのソケットアドレス。
    fn remote_address(&self) -> std::net::SocketAddr;

    /// 接続をクローズする (= アプリケーションレベルの close)。
    fn close(&self, code: u32, reason: &[u8]);
}
