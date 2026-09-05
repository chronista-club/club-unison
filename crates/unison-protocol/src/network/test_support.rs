//! test 専用の [`UnisonConn`] mock。
//!
//! `handle_connection` / `ProtocolServer` の接続台帳まわりを、 実 QUIC を立てずに
//! unit test するための最小実装。 `accept_bi` / `open_bi` の振る舞いを caller が
//! 決められる。

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::NetworkError;
use super::conn::{BiStream, UnisonConn};

/// 何もしない mock 接続。
///
/// - `accept_bi` — 即 `Err` (= 接続終了) を返し、 caller のループを 1 周で抜けさせる
/// - `open_bi` — `Err` (identity 送信は warn されて継続する)
/// - `recv_datagram` — 永久に pending (= dispatcher の recv task を生かしたままにする)
/// - `send_datagram` — 数えるだけで `Ok`
pub(crate) struct MockConn {
    remote: SocketAddr,
    datagrams_sent: Arc<AtomicUsize>,
}

impl MockConn {
    pub(crate) fn new(remote: SocketAddr) -> Self {
        Self {
            remote,
            datagrams_sent: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 送信された datagram 数のカウンタを共有する。
    pub(crate) fn datagram_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.datagrams_sent)
    }
}

impl UnisonConn for MockConn {
    fn accept_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>
    {
        Box::pin(async { Err(NetworkError::Connection("mock: closed".to_string())) })
    }

    fn open_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>
    {
        Box::pin(async { Err(NetworkError::Connection("mock: no streams".to_string())) })
    }

    fn send_datagram(&self, _data: bytes::Bytes) -> Result<(), NetworkError> {
        self.datagrams_sent.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn recv_datagram(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bytes::Bytes, NetworkError>> + Send + '_>>
    {
        // 接続が生きている限り datagram は来ない、 という状態を再現する。
        Box::pin(std::future::pending())
    }

    fn remote_address(&self) -> SocketAddr {
        self.remote
    }

    fn close(&self, _code: u32, _reason: &[u8]) {}
}
