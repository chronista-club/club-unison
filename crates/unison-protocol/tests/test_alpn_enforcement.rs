//! ALPN enforcement の回帰テスト (v1.3.0+)。
//!
//! QUIC は RFC 9001 §8.1 で ALPN を必須とし、rustls/quinn は plain TLS と違い
//! handshake 時に enforce する。`network::UNISON_ALPN` (= `"unison"`) を設定した
//! server に対し:
//! - **ALPN を送る client** は接続できる（positive）。
//! - **ALPN を送らない旧 client** は `no_application_protocol` で拒否される（negative）。
//!
//! この非対称が「v1.3.0 は raw QUIC client にとって breaking（後方互換ではない）」の
//! 根拠であり、Ruby gem 等の旧 client は再ビルドが必要。詳細は
//! `design/quic-runtime.md` の ALPN 節。
//!
//! handshake のみで完結するため軽量・決定論的。CI でも走らせて契約を守る
//! (= 他の Medium QUIC test と違い `#[ignore]` を付けない)。

use std::sync::Arc;
use std::time::Duration;

use quinn::{ClientConfig, Endpoint};
use rustls::ClientConfig as RustlsClientConfig;
use tokio::time::timeout;

use unison::network::trust::TrustAnchors;
use unison::network::{ProtocolServer, QuicServer};

/// dev_localhost cert（= ALPN "unison" が自動設定される）の server を spawn し、
/// 接続用アドレスを返す。
async fn spawn_server() -> String {
    let server = Arc::new(ProtocolServer::with_identity("alpn-test", "1.0.0", "test"));
    let mut quic = QuicServer::new(Arc::clone(&server));
    quic.bind("[::1]:0").await.expect("bind");
    let local = quic.local_addr().expect("local_addr");
    let addr = format!("[{}]:{}", local.ip(), local.port());
    tokio::spawn(async move {
        let _ = quic.start().await;
    });
    addr
}

/// 指定 ALPN で client endpoint を作り、server へ接続を試みる。
async fn try_connect(addr: &str, alpn: &[&str]) -> Result<(), String> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let mut rustls_cfg: RustlsClientConfig = (*TrustAnchors::SkipVerification
        .build_client_config()
        .unwrap())
    .clone();
    // テストしたい ALPN で上書き（空配列 = 旧 client を再現）。
    rustls_cfg.alpn_protocols = alpn.iter().map(|s| s.as_bytes().to_vec()).collect();

    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_cfg)
        .map_err(|e| format!("client crypto config: {e}"))?;
    let client_config = ClientConfig::new(Arc::new(crypto));

    let mut endpoint = Endpoint::client("[::]:0".parse().unwrap()).map_err(|e| e.to_string())?;
    endpoint.set_default_client_config(client_config);

    let sockaddr: std::net::SocketAddr = addr
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let result = timeout(Duration::from_secs(5), async {
        endpoint
            .connect(sockaddr, "localhost")
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "connect timeout".to_string())?;

    result.map(|_conn| ())
}

#[tokio::test]
async fn unison_alpn_client_connects() {
    let addr = spawn_server().await;
    let result = try_connect(&addr, &["unison"]).await;
    assert!(
        result.is_ok(),
        "\"unison\" ALPN を送る client は接続できるべき: {result:?}"
    );
}

#[tokio::test]
async fn empty_alpn_client_is_rejected() {
    let addr = spawn_server().await;
    let result = try_connect(&addr, &[]).await;
    // QUIC は ALPN 必須 → ALPN 無しの旧 client は handshake で弾かれる。
    assert!(
        result.is_err(),
        "ALPN を送らない旧 client は拒否されるべき (後方互換ではない)"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("protocol") || err.contains("handshake") || err.contains("120"),
        "no_application_protocol 由来の handshake 失敗を期待: {err}"
    );
}

#[tokio::test]
async fn mismatched_alpn_client_is_rejected() {
    let addr = spawn_server().await;
    // server は "unison" のみ。別 label を出す client は negotiate 失敗。
    let result = try_connect(&addr, &["h3"]).await;
    assert!(
        result.is_err(),
        "server の ALPN と一致しない client は拒否されるべき"
    );
}
