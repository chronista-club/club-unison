//! `quinn` 型を transport 抽象 ([`UnisonConn`] / [`UnisonSend`] / [`UnisonRecv`])
//! に橋渡しする adapter impl。
//!
//! `quinn::Connection` / `SendStream` / `RecvStream` を raw QUIC backend として
//! [`UnisonConn`] family へ写す。 WebTransport backend は [`super::webtransport`]
//! 側に同種の adapter を持つ。

use std::net::SocketAddr;
use std::pin::Pin;

use quinn::{Connection, RecvStream, SendStream};

use super::NetworkError;
use super::conn::{BiStream, BoxUnisonRecv, BoxUnisonSend, UnisonConn, UnisonRecv, UnisonSend};

/// `quinn::SendStream` を [`UnisonSend`] として扱う。
///
/// `quinn::SendStream` は `tokio::io::AsyncWrite` を実装済みなので、 `finish`
/// だけ橋渡しすればよい。
impl UnisonSend for SendStream {
    fn finish(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), NetworkError>> + Send + '_>> {
        // quinn の finish は同期。 trait の async シグネチャに合わせて即解決する
        // future で包む。 二重 finish (= ClosedStream) は正常終了扱い (= 冪等)。
        let result = match SendStream::finish(self) {
            Ok(()) | Err(quinn::ClosedStream { .. }) => Ok(()),
        };
        Box::pin(async move { result })
    }
}

/// `quinn::RecvStream` を [`UnisonRecv`] として扱う。
impl UnisonRecv for RecvStream {
    fn stop(&mut self) -> Result<(), NetworkError> {
        match RecvStream::stop(self, quinn::VarInt::from_u32(0)) {
            Ok(()) | Err(quinn::ClosedStream { .. }) => Ok(()),
        }
    }
}

/// `quinn::Connection` を transport-agnostic な [`UnisonConn`] として公開する。
impl UnisonConn for Connection {
    fn accept_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>
    {
        Box::pin(async move {
            let (send, recv) = Connection::accept_bi(self)
                .await
                .map_err(|e| NetworkError::Quic(format!("accept_bi failed: {}", e)))?;
            let send: BoxUnisonSend = Box::new(send);
            let recv: BoxUnisonRecv = Box::new(recv);
            Ok((send, recv))
        })
    }

    fn open_bi(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<BiStream, NetworkError>> + Send + '_>>
    {
        Box::pin(async move {
            let (send, recv) = Connection::open_bi(self)
                .await
                .map_err(|e| NetworkError::Quic(format!("open_bi failed: {}", e)))?;
            let send: BoxUnisonSend = Box::new(send);
            let recv: BoxUnisonRecv = Box::new(recv);
            Ok((send, recv))
        })
    }

    fn send_datagram(&self, data: bytes::Bytes) -> Result<(), NetworkError> {
        Connection::send_datagram(self, data)
            .map_err(|e| NetworkError::Quic(format!("send_datagram failed: {}", e)))
    }

    fn recv_datagram(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<bytes::Bytes, NetworkError>> + Send + '_>>
    {
        Box::pin(async move {
            Connection::read_datagram(self)
                .await
                .map_err(|e| NetworkError::Quic(format!("recv_datagram failed: {}", e)))
        })
    }

    fn remote_address(&self) -> SocketAddr {
        Connection::remote_address(self)
    }

    fn close(&self, code: u32, reason: &[u8]) {
        Connection::close(self, quinn::VarInt::from_u32(code), reason);
    }
}
