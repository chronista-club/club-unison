//! ping/pong (1 request / 1 response) round-trip bench
//!
//! `UnisonChannel::request<Req, Resp>()` の 1 req / 1 resp 往復 latency を payload
//! size 4 段階 (16 / 64 / 256 / 1024 B) で測る。 これは「**通常の 1 リクエスト・
//! レスポンス**」 baseline で、 throughput 系 (= batch/burst/stream) との対比に使う。
//!
//! 注: 各 iter で server + client + connection を新規 setup する simple form
//! (= setup overhead 込み)。 純粋な round-trip latency を厳密に取りたい場合は
//! v0.10+ で setup 1 回 + iter は req/resp のみの form に refine 予定。

use unison::network::channel::UnisonChannel;
use unison::network::{MessageType, quic::QuicClient};
use unison::{ProtocolClient, ProtocolServer};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde_json::json;
use std::hint::black_box;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;

/// payload size variations (small / common / mid / large)
const PING_PAYLOAD_SIZES: &[usize] = &[16, 64, 256, 1024];

/// 同 bench 内で port 衝突を避けるための counter (start: 25000)
static PORT_COUNTER: AtomicU16 = AtomicU16::new(0);

async fn setup_ping_server(port: u16) {
    tokio::spawn(async move {
        let server = ProtocolServer::new();
        server
            .register_channel("ping", |_ctx, stream| async move {
                let channel: UnisonChannel = UnisonChannel::new(stream);
                while let Ok(msg) = channel.recv().await {
                    if msg.msg_type == MessageType::Request {
                        let payload = msg.payload_as_value().unwrap_or_default();
                        if channel
                            .send_response(msg.id, &msg.method, &payload)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Ok(())
            })
            .await;
        let _ = server.listen(&format!("[::1]:{}", port)).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    // server bind 完了を待つ
    tokio::time::sleep(Duration::from_millis(150)).await;
}

fn bench_ping_pong(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();
    let mut group = c.benchmark_group("ping_pong");
    group.measurement_time(Duration::from_secs(5));

    for &size in PING_PAYLOAD_SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.to_async(&runtime).iter(|| async move {
                let port = 25000 + PORT_COUNTER.fetch_add(1, Ordering::Relaxed);
                setup_ping_server(port).await;

                let quic_client = QuicClient::new().unwrap();
                let client = ProtocolClient::new(quic_client);
                client.connect(&format!("[::1]:{}", port)).await.unwrap();

                let channel = client.open_channel("ping").await.unwrap();
                let payload = json!({ "data": "x".repeat(size) });
                let result: Result<serde_json::Value, _> = channel.request("ping", &payload).await;

                black_box(result.is_ok())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_ping_pong);
criterion_main!(benches);
