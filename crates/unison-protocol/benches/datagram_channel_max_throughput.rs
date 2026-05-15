//! datagram_channel_max_throughput bench — 限界値計測 (= rate-limit なし)
//!
//! # 目的
//!
//! 「このマシン (= 計測 host) で datagram channel API は何 msg/s 出せるか」 の **上限**
//! を求める。 既存 sustained bench (= 60Hz / 120Hz rate-limited) は「目標 rate に追従
//! できるか」 を verify する caller 視点、 本 bench は「library / system が出せる ceiling」
//! を露呈する system 視点。
//!
//! # Bench shape
//!
//! - Setup: 1 server + 1 client + 1 channel (= sustained と同 pattern)
//! - iter: fixed duration (= 2 sec) で **as-fast-as-possible** で Transform 送信
//! - send / recv 別 task (= Arc<DatagramChannel>)、 send loop は rate limit なしの tight loop
//! - Metric: sent / sec, received / sec, drop rate
//!
//! # 出力
//!
//! criterion は session 所要時間 (= ≈ STREAM_DURATION + setup) を測るのみ、
//! 本命 metric は `eprintln!` 経由 stderr 出力で raw 数字を流す:
//!
//! ```text
//! [max iters=N] sent=X server_recv=Y client_recv=Z drop=W% sent/s=A recv/s=B
//! ```

use club_unison::{ProtocolClient, ProtocolServer};
use criterion::{Criterion, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;

/// 1 session の stream 持続時間 (= sustained と同じ、 比較しやすい)
const STREAM_DURATION: Duration = Duration::from_secs(2);

/// server spawn / channel open の non-blocking gap
const SETUP_GAP: Duration = Duration::from_millis(50);

/// recv loop の poll timeout
const RECV_POLL_TIMEOUT: Duration = Duration::from_millis(10);

/// session_deadline 後の recv 余裕
const RECV_TAIL_BUFFER: Duration = Duration::from_millis(300);

/// 位置同期 Transform (= sustained と同 struct、 比較性確保)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Transform {
    id: String,
    pos: [f32; 3],
    rot: [f32; 4],
}

fn bench_max_throughput(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("datagram_channel_max_throughput");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(8));

    group.bench_function("unlimited_rate_dur_2sec", |b| {
        b.to_async(&runtime).iter_custom(|iters| async move {
            // ─── Setup ───
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

            // ─── Measure ───
            let mut total_sent = 0_usize;
            let mut total_received = 0_usize;
            let start = std::time::Instant::now();
            for _ in 0..iters {
                let transform = Transform {
                    id: "bench-peer".to_string(),
                    pos: [1.0, 2.0, 3.0],
                    rot: [0.0, 0.0, 0.0, 1.0],
                };

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

                // send loop: rate 制限なし、 as-fast-as-possible
                let session_start = std::time::Instant::now();
                let mut session_sent = 0_usize;
                while session_start.elapsed() < STREAM_DURATION {
                    if chan.send_event(&transform).await.is_ok() {
                        session_sent += 1;
                    }
                }

                let _ = recv_task.await;
                let session_received = recv_count.load(Ordering::Relaxed);

                total_sent += session_sent;
                total_received += session_received;
                black_box((session_sent, session_received));
            }
            let elapsed = start.elapsed();

            // ─── Cleanup ───
            drop(chan);
            let _ = client.disconnect().await;
            let _ = handle.shutdown().await;

            // Side-channel stats
            let total_handler_echoes = server_echo_count.load(Ordering::Relaxed);
            let drop_pct = if total_sent > 0 {
                100.0 * (1.0 - (total_received as f64 / total_sent as f64))
            } else {
                0.0
            };
            // 両 metric を **同じ分母 (= STREAM_DURATION × iters)** で計算、 「send window
            // 中の throughput」 を表す。 こうすると `1 - recv/sent` が drop rate と
            // algebraically 一致、 reader の cognitive load 削減。
            let send_window_secs = STREAM_DURATION.as_secs_f64() * iters as f64;
            let sent_per_sec = total_sent as f64 / send_window_secs;
            let recv_per_sec = total_received as f64 / send_window_secs;
            eprintln!(
                "[max iters={}] sent={} server_recv={} client_recv={} drop={:.1}% sent/s={:.0} recv/s={:.0}",
                iters,
                total_sent,
                total_handler_echoes,
                total_received,
                drop_pct,
                sent_per_sec,
                recv_per_sec,
            );

            elapsed
        });
    });

    group.finish();
}

criterion_group!(benches, bench_max_throughput);
criterion_main!(benches);
