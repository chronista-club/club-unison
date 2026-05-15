//! datagram bench — 3DCG position+rotation 大量配信 scenario (unison MVP dogfood)
//!
//! # 想定ユースケース
//!
//! 3DCG 空間の同期 (= multi-user VR / metaverse / 3D sandbox 等) で
//! **position + rotation transform** を高頻度・大量配信する用途。
//!
//! - **payload size 2 ケース**:
//!   - 64 B: 1 つの transform (= 3 float position + 4 float quaternion ≈ 28B raw、
//!     serialize 込み 64B 想定) を 1 packet に収める典型ケース
//!   - 1300 B: MTU 安全値上限 (= IP MTU 1500 - IP/UDP/QUIC header ≈ 1300)、 1 packet
//!     で運べる最大 payload。 multiple transform pack や厚めの payload 用途
//! - **burst pattern**: 1 frame (= 60Hz / 120Hz) あたり N 個の peer / object の
//!   transform を一斉配信 (= 100 / 1000 = 中規模 / 大規模シーン)
//! - **unreliable / unordered で OK**: 古い transform は新しい transform で上書き
//! - **MTU 内 (≤1300B)** で IP fragment 回避、 latency 安定
//!
//! # 設計指針との関係
//!
//! v0.9.0 で `QuicClient::send_datagram` / `recv_datagram` MVP API を新設、 これは
//! connection-level thin wrapper (= channel 抽象は経由しない、 caller が demux header
//! を payload に含める責任)。 v0.10+ で `event "X" backend="datagram"` KDL schema
//! 拡張と一緒に channel API へ統合予定 (= `design/wire-format.md` 参照)。
//!
//! # 実装メモ
//!
//! - **client = unison QuicClient (MVP dogfood)**、 **server = raw quinn Endpoint**
//!   (= server 側 datagram echo は v0.10+ で unison QuicServer に統合予定、 今は
//!   raw で済ませる)
//! - 各 iter で server + client + connection を新規 setup (= simple form、 setup
//!   込み measurement)
//! - burst send 後、 recv_datagram で echo 受信 count を黒箱化 (= unreliable で
//!   loss あり、 timeout で打ち切り)

use club_unison::network::quic::QuicClient;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use quinn::{Endpoint, ServerConfig, TransportConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::hint::black_box;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;

/// payload size 2 ケース (= 1 transform / MTU max)
const PAYLOAD_SIZES: &[usize] = &[64, 1300];

/// burst 連続送信数 (= 1 frame で N 個の peer の transform を broadcast 想定)
const BURST_COUNTS: &[usize] = &[100, 1000];

/// echo recv の timeout (= unreliable datagram で全 N 個戻る保証なし)
const RECV_TIMEOUT: Duration = Duration::from_millis(500);

static PORT_COUNTER: AtomicU16 = AtomicU16::new(0);

/// raw quinn server (= datagram echo)、 v0.10+ で unison QuicServer 統合予定
fn make_server_endpoint(port: u16) -> Endpoint {
    let cert =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("rcgen self-signed");
    let cert_der = CertificateDer::from(cert.cert.der().to_vec());
    let key_pkcs8 = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let key = PrivateKeyDer::Pkcs8(key_pkcs8);

    // datagram 有効化 (= unison QuicClient 側と整合)
    let mut transport = TransportConfig::default();
    transport.datagram_receive_buffer_size(Some(1024 * 1024));
    transport.datagram_send_buffer_size(1024 * 1024);

    let mut server_config =
        ServerConfig::with_single_cert(vec![cert_der], key).expect("server tls config");
    server_config.transport_config(Arc::new(transport));

    let addr: SocketAddr = (Ipv6Addr::LOCALHOST, port).into();
    let endpoint = Endpoint::server(server_config, addr).expect("server endpoint bind");

    // accept + datagram echo loop
    let endpoint_clone = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = endpoint_clone.accept().await {
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    while let Ok(datagram) = conn.read_datagram().await {
                        let _ = conn.send_datagram(datagram);
                    }
                }
            });
        }
    });

    endpoint
}

fn bench_datagram_burst(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("datagram_burst_3dcg");
    group.measurement_time(Duration::from_secs(5));

    for &payload_size in PAYLOAD_SIZES {
        for &burst_count in BURST_COUNTS {
            let bench_id = format!("payload_{}_burst_{}", payload_size, burst_count);
            group.bench_with_input(
                BenchmarkId::from_parameter(&bench_id),
                &(payload_size, burst_count),
                |b, &(payload_size, burst_count)| {
                    b.to_async(&runtime).iter(|| async move {
                        let port = 26000 + PORT_COUNTER.fetch_add(1, Ordering::Relaxed);
                        let _server = make_server_endpoint(port);

                        // unison MVP API: QuicClient direct (= channel 抽象 bypass、
                        // datagram は connection-level)
                        let quic_client = QuicClient::new().expect("QuicClient::new");
                        quic_client
                            .connect(&format!("[::1]:{}", port))
                            .await
                            .expect("connect");

                        // burst send (= 1 frame で N peer の transform を一斉配信)
                        let payload = bytes::Bytes::from(vec![0xab_u8; payload_size]);
                        for _ in 0..burst_count {
                            let _ = quic_client.send_datagram(payload.clone()).await;
                        }

                        // echo recv (= unreliable で loss あり、 timeout で打ち切り)
                        let mut received = 0_usize;
                        while received < burst_count {
                            match tokio::time::timeout(RECV_TIMEOUT, quic_client.recv_datagram())
                                .await
                            {
                                Ok(Ok(_)) => received += 1,
                                _ => break,
                            }
                        }

                        black_box(received)
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_datagram_burst);
criterion_main!(benches);
