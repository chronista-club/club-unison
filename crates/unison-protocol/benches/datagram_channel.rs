//! datagram_channel bench — v0.10.0 channel API 経由の datagram throughput
//!
//! # 目的
//!
//! v0.10.0 で導入された **channel API datagram path** (= `ProtocolServer::register_channel_datagram` +
//! `ProtocolClient::open_datagram_channel`) のオーバーヘッドを計測する。 既存
//! `benches/datagram.rs` (= connection-level raw `QuicClient::send_datagram` + raw quinn
//! echo server) と **同じ payload (64 / 1300 B) × burst (100 / 1000)** で並列に測り、
//! RESULTS.md 上で「raw vs channel API」 の数値 diff を可視化する。
//!
//! # 計測する overhead 要素
//!
//! channel API path は raw path に対して以下を追加で行う:
//!
//! 1. **JSON codec encode/decode** (= `chan.send_event::<Payload>` 内で `serde_json::to_vec`、
//!    recv 側で `serde_json::from_slice`)
//! 2. **Varint channel_id prefix** (= 1-byte for channel_id=1) の encode / decode
//! 3. **`DatagramDispatcher` 経由の routing** (= per-connection background task が
//!    `read_datagram` → `decode_varint` → `mpsc::Sender::try_send` で channel mpsc に流す)
//! 4. **`DatagramChannel::recv_event` 経由の pull** (= `mpsc::Receiver` から payload を取る)
//! 5. **server side handler invocation** (= `tokio::spawn` された echo handler 内で
//!    `chan.recv_event<Payload>` → `chan.send_event<Payload>`)
//!
//! ## 設計 trade-off (= 既存 datagram.rs との 公平比較性)
//!
//! 両 bench は `iter_custom` で **1 connection を全 iter で共有する steady-state**
//! pattern (= setup overhead を measurement から除外、 v0.10.1 で cold-start per-iter
//! から切替、 macOS の ephemeral port / fd 枯渇問題を回避)。 raw bench との diff は
//! 概ね「channel encode/decode + dispatcher routing + JSON cost」 の合計、 burst 1000
//! で JSON encoding の cost が顕在化する想定。

use unison::{ProtocolClient, ProtocolServer};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

/// payload size 2 ケース (= 1 transform / MTU max 想定、 datagram.rs と統一)
const PAYLOAD_SIZES: &[usize] = &[64, 1300];

/// burst 連続送信数 (= 1 frame で N peer を broadcast、 datagram.rs と統一)
const BURST_COUNTS: &[usize] = &[100, 1000];

/// echo recv timeout (= unreliable + JSON overhead で raw より長め)
const RECV_TIMEOUT: Duration = Duration::from_millis(500);

/// server spawn と client connect の間の non-blocking setup gap
const SETUP_GAP: Duration = Duration::from_millis(50);

/// Bench payload (= JSON serializable、 channel API は default JsonCodec を使用)
///
/// `data: Vec<u8>` を持つ単純構造、 JSON wire 上は `{"data":[171,...,171]}` 形式で
/// raw bytes に対し ~3-5x の overhead がある (= channel API の現実的な用途を反映)。
/// ProtoCodec でより compact にする path は将来 codec generic bench で対応。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Payload {
    data: Vec<u8>,
}

fn bench_datagram_channel_burst(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("datagram_channel_burst_3dcg");
    // v0.10.1: per-iter cold-start (= 1 connection / iter) は macOS の ephemeral port /
    // fd 枯渇に詰まるため、 1 connection + 1 channel を作って iter 全部で再利用、
    // steady-state burst を測る semantic に切替 (= datagram.rs bench と同 pattern)。
    // RESULTS.md で「steady-state、 cold start ではない」 を明示する。
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(3));

    for &payload_size in PAYLOAD_SIZES {
        for &burst_count in BURST_COUNTS {
            let bench_id = format!("payload_{}_burst_{}", payload_size, burst_count);
            group.bench_with_input(
                BenchmarkId::from_parameter(&bench_id),
                &(payload_size, burst_count),
                |b, &(payload_size, burst_count)| {
                    b.to_async(&runtime).iter_custom(|iters| async move {
                        // ─── Setup (= 1 connection + 1 channel、 iter で再利用) ───
                        let server = Arc::new(ProtocolServer::new());
                        server
                            .register_channel_datagram("position", 1, |chan| async move {
                                loop {
                                    match chan.recv_event::<Payload>().await {
                                        Ok(p) => {
                                            let _ = chan.send_event(&p).await;
                                        }
                                        Err(_) => break,
                                    }
                                }
                            })
                            .await;
                        let handle = Arc::clone(&server)
                            .spawn_listen_shared("[::1]:0")
                            .await
                            .expect("spawn_listen_shared");
                        let server_addr = handle.local_addr();

                        let client = ProtocolClient::new_default().expect("client::new_default");
                        client
                            .connect(&format!("[{}]:{}", server_addr.ip(), server_addr.port()))
                            .await
                            .expect("client::connect");

                        tokio::time::sleep(SETUP_GAP).await;

                        let chan = client
                            .open_datagram_channel("position", 1)
                            .await
                            .expect("open_datagram_channel");

                        let payload = Payload {
                            data: vec![0xab_u8; payload_size],
                        };

                        // ─── Measure: iters 回の burst (= steady-state) ───
                        let start = std::time::Instant::now();
                        for _ in 0..iters {
                            for _ in 0..burst_count {
                                let _ = chan.send_event(&payload).await;
                            }
                            let mut received = 0_usize;
                            while received < burst_count {
                                match tokio::time::timeout(
                                    RECV_TIMEOUT,
                                    chan.recv_event::<Payload>(),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => received += 1,
                                    _ => break,
                                }
                            }
                            black_box(received);
                        }
                        let elapsed = start.elapsed();

                        // Cleanup (= bench 終了で server / client / dispatcher を drop)
                        let _ = client.disconnect().await;
                        let _ = handle.shutdown().await;

                        elapsed
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_datagram_channel_burst);
criterion_main!(benches);
