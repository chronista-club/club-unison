//! datagram_channel_sustained bench — 位置同期 use case (= 60Hz / 120Hz × 数秒 stream)
//!
//! # 目的
//!
//! 「3DCG / metaverse の位置同期」 の **realistic use case shape** を bench する。
//! 1 connection を確立後、 fixed duration (= 2 sec) で transform を一定 rate で送り続け、
//! steady-state throughput と drop rate を計測する。
//!
//! 既存 `datagram_channel.rs` (= 1 frame multicast burst) とは bench shape が違う:
//!
//! - **burst**: 1 frame で N peer に as-fast-as-possible 連射、 「single tick の速度上限」
//! - **sustained** (= 本 file): 60Hz / 120Hz の **rate-limited 連続 stream**、 「現実的な
//!   position sync の継続安定動作」、 game / 3DCG の本命 use case
//!
//! # Metrics
//!
//! criterion 標準 = session 所要時間 (= ≈ STREAM_DURATION + setup)。 加えて `eprintln!`
//! で各 case の **sent / received / drop% / effective msg/s** を stderr 出力、
//! RESULTS.md に集約。
//!
//! # 設計
//!
//! - `Arc<DatagramChannel>` で send / recv を 別 task に分離 (= sustained streaming の
//!   realistic な並列 pattern)
//! - send task は `tokio::time::interval(1/rate)` で rate-limited
//! - recv task は session_deadline まで polling、 timeout per poll で busy-loop 回避
//! - Transform struct (= peer id + pos + rot) JSON wire ~110-130 byte、 MTU 内

use unison::{ProtocolClient, ProtocolServer};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;

/// 60Hz / 120Hz の 2 ケース (= game / VR で典型的な refresh rate)
const TARGET_RATES_HZ: &[u64] = &[60, 120];

/// 1 session の stream 持続時間
const STREAM_DURATION: Duration = Duration::from_secs(2);

/// server spawn 後 client が channel open する間の non-blocking gap
const SETUP_GAP: Duration = Duration::from_millis(50);

/// recv loop の poll timeout (= drop tolerance、 次 frame まで待つ時間)
const RECV_POLL_TIMEOUT: Duration = Duration::from_millis(20);

/// session_deadline 後の recv 余裕 (= last sent frame が回ってくるまで)
const RECV_TAIL_BUFFER: Duration = Duration::from_millis(300);

/// 位置同期 payload — 3DCG transform (= position + rotation)
///
/// JSON wire size ≈ 110-130 byte (= peer id + pos f32×3 + rot f32×4)、 MTU 1300 内。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Transform {
    id: String,
    pos: [f32; 3],
    rot: [f32; 4],
}

fn bench_sustained_position_sync(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("datagram_channel_sustained_position_sync");
    // 1 session = ~2 sec + recv tail、 measurement_time 8 sec で 2-3 session 測れる
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(8));

    for &rate_hz in TARGET_RATES_HZ {
        let case_id = format!("rate_{}hz_dur_{}sec", rate_hz, STREAM_DURATION.as_secs());
        let expected_sends_per_session = rate_hz * STREAM_DURATION.as_secs();

        group.bench_with_input(
            BenchmarkId::from_parameter(&case_id),
            &rate_hz,
            |b, &rate_hz| {
                b.to_async(&runtime).iter_custom(|iters| async move {
                    // ─── Setup: 1 connection + 1 channel、 全 iter で共有 ───
                    let server = Arc::new(ProtocolServer::new());
                    let server_echo_count = Arc::new(AtomicUsize::new(0));
                    let server_echo_count_h = Arc::clone(&server_echo_count);
                    server
                        .register_channel_datagram("position", 1, move |chan| {
                            let counter = Arc::clone(&server_echo_count_h);
                            async move {
                                loop {
                                    match chan.recv_event::<Transform>().await {
                                        Ok(t) => {
                                            counter.fetch_add(1, Ordering::Relaxed);
                                            let _ = chan.send_event(&t).await;
                                        }
                                        Err(_) => break,
                                    }
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

                    let chan = Arc::new(
                        client
                            .open_datagram_channel("position", 1)
                            .await
                            .expect("open_datagram_channel"),
                    );

                    let interval_period = Duration::from_nanos(1_000_000_000 / rate_hz);

                    // ─── Measure: iters 回の sustained session ───
                    let mut total_sent = 0_usize;
                    let mut total_received = 0_usize;
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let transform = Transform {
                            id: "bench-peer".to_string(),
                            pos: [1.0, 2.0, 3.0],
                            rot: [0.0, 0.0, 0.0, 1.0],
                        };

                        // recv task を session 間並行で走らせる
                        let chan_recv = Arc::clone(&chan);
                        let recv_count = Arc::new(AtomicUsize::new(0));
                        let recv_count_h = Arc::clone(&recv_count);
                        let session_deadline =
                            std::time::Instant::now() + STREAM_DURATION + RECV_TAIL_BUFFER;
                        let recv_task = tokio::spawn(async move {
                            while std::time::Instant::now() < session_deadline {
                                match tokio::time::timeout(
                                    RECV_POLL_TIMEOUT,
                                    chan_recv.recv_event::<Transform>(),
                                )
                                .await
                                {
                                    Ok(Ok(_)) => {
                                        recv_count_h.fetch_add(1, Ordering::Relaxed);
                                    }
                                    _ => {
                                        // poll timeout、 次 frame 待ち
                                    }
                                }
                            }
                        });

                        // send loop: rate-limited、 STREAM_DURATION 経過まで
                        let session_start = std::time::Instant::now();
                        let mut interval = tokio::time::interval(interval_period);
                        // interval は first tick が即発火するので skip (= 仕様)
                        let mut session_sent = 0_usize;
                        while session_start.elapsed() < STREAM_DURATION {
                            interval.tick().await;
                            if chan.send_event(&transform).await.is_ok() {
                                session_sent += 1;
                            }
                        }

                        // recv task 終了待ち
                        let _ = recv_task.await;
                        let session_received = recv_count.load(Ordering::Relaxed);

                        total_sent += session_sent;
                        total_received += session_received;
                        black_box((session_sent, session_received));
                    }
                    let elapsed = start.elapsed();

                    // ─── Cleanup ───
                    drop(chan); // Arc<DatagramChannel> を release
                    let _ = client.disconnect().await;
                    let _ = handle.shutdown().await;

                    // Side-channel stats (= stderr、 criterion とは別経路で raw 出力)
                    let total_handler_echoes = server_echo_count.load(Ordering::Relaxed);
                    let drop_pct = if total_sent > 0 {
                        100.0 * (1.0 - (total_received as f64 / total_sent as f64))
                    } else {
                        0.0
                    };
                    let effective_recv_per_sec =
                        total_received as f64 / elapsed.as_secs_f64();
                    eprintln!(
                        "[sustained {}hz iters={}] sent={} server_recv={} client_recv={} drop={:.1}% effective_recv_msg/s={:.1} (expected_send/session={})",
                        rate_hz,
                        iters,
                        total_sent,
                        total_handler_echoes,
                        total_received,
                        drop_pct,
                        effective_recv_per_sec,
                        expected_sends_per_session,
                    );

                    elapsed
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_sustained_position_sync);
criterion_main!(benches);
