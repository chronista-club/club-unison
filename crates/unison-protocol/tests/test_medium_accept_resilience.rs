//! accept loop resilience の回帰テスト。
//!
//! **バグ (2026-07-13 特定)**: `QuicServer::start()` / `start_with_shutdown()` の accept loop が
//! `let connection = connecting.await?;` を **spawn の前・loop の中**で await していた。
//! incoming handshake が 1 つでも失敗すると `?` が accept loop 全体を終了させ、以降の新規接続を
//! 一切受け付けなくなる (既存の spawn 済み接続は生存 = 「片肺死」)。
//!
//! 実害: VP daemon で federation direct-dial (peer が dev cert の :32000 へ System trust で接続 →
//! `UnknownIssuer` で handshake abort) が incoming failed handshake を生み、acceptor を殺していた。
//! daemon.kdl.log に「Daemon Unison サーバーエラー: ...handshake failed...」→ watchdog self-heal
//! の連鎖が 10+ 回記録 (creo mem_1CcvYA5TRF4EcFafbyKqPg)。
//!
//! このテストは「**失敗した handshake の後も acceptor が新規接続を受け続ける**」ことを保証する。
//! 失敗 handshake は ALPN mismatch (空 ALPN の旧 client 相当) で決定論的に起こす
//! (test_alpn_enforcement と同じ機序、handshake のみで完結 = 軽量)。

use std::sync::Arc;
use std::time::Duration;

use quinn::{ClientConfig, Endpoint};
use rustls::ClientConfig as RustlsClientConfig;
use tokio::time::timeout;

use unison::network::trust::TrustAnchors;
use unison::network::{ProtocolServer, QuicServer};

/// dev_localhost cert の server を spawn し、接続用アドレスを返す。
async fn spawn_server() -> String {
    let server = Arc::new(ProtocolServer::with_identity(
        "accept-resilience",
        "1.0.0",
        "test",
    ));
    let mut quic = QuicServer::new(Arc::clone(&server));
    quic.bind("[::1]:0").await.expect("bind");
    let local = quic.local_addr().expect("local_addr");
    let addr = format!("[{}]:{}", local.ip(), local.port());
    tokio::spawn(async move {
        let _ = quic.start().await;
    });
    addr
}

/// 指定 ALPN で client を作り server へ接続を試みる (handshake まで)。
/// 空 ALPN = handshake 失敗 (旧 client 相当)、`["unison"]` = 成功。
async fn try_connect(addr: &str, alpn: &[&str]) -> Result<(), String> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let mut rustls_cfg: RustlsClientConfig = (*TrustAnchors::SkipVerification
        .build_client_config()
        .unwrap())
    .clone();
    rustls_cfg.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();

    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_cfg)
        .map_err(|e| format!("client crypto config: {e}"))?;
    let client_config = ClientConfig::new(Arc::new(crypto));

    let mut endpoint = Endpoint::client("[::]:0".parse().unwrap()).map_err(|e| e.to_string())?;
    endpoint.set_default_client_config(client_config);

    let sockaddr: std::net::SocketAddr = addr
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    timeout(Duration::from_secs(5), async {
        endpoint
            .connect(sockaddr, "localhost")
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "connect timeout".to_string())?
    .map(|_conn| ())
}

/// **回帰**: 失敗した handshake は当該接続に閉じ、accept loop を殺してはならない。
/// バグ版 (`connecting.await?` が loop 内) では 3 回目の good connect が timeout する。
#[tokio::test]
async fn acceptor_survives_failed_handshake() {
    let addr = spawn_server().await;

    // 1. 正常 client → 接続成立 (acceptor 生存の sanity)
    let first = try_connect(&addr, &["unison"]).await;
    assert!(first.is_ok(), "初回の good connect は成立すべき: {first:?}");

    // 2. 空 ALPN client → handshake 失敗 (これが acceptor を殺してはならない)
    let bad = try_connect(&addr, &[]).await;
    assert!(bad.is_err(), "空 ALPN client は handshake 拒否されるべき");

    // 失敗 handshake を server 側 accept loop が処理しきる猶予 (buggy 版なら start() が exit)。
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 3. 再び正常 client → **まだ**接続できること (バグ版はここで timeout)。
    let after = try_connect(&addr, &["unison"]).await;
    assert!(
        after.is_ok(),
        "失敗 handshake の後も acceptor は新規接続を受け続けるべき（accept loop が死んでいる）: {after:?}"
    );
}
