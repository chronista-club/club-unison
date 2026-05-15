use club_unison::network::channel::UnisonChannel;
use club_unison::network::{MessageType, quic::QuicClient};
use club_unison::{ProtocolClient, ProtocolServer};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hdrhistogram::Histogram;
use serde_json::json;
use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Barrier;

/// メッセージサイズのバリエーション
const MESSAGE_SIZES: &[usize] = &[64, 256, 1024, 4096, 16384];

/// レイテンシ測定用の関数
async fn measure_latency(
    channel: &UnisonChannel,
    message_size: usize,
    iterations: u32,
) -> Histogram<u64> {
    let mut histogram = Histogram::<u64>::new(3).unwrap();

    for i in 0..iterations {
        let message = json!({
            "data": "x".repeat(message_size),
            "sequence": i
        });

        let start = std::time::Instant::now();
        let _ = channel
            .request::<_, serde_json::Value>("echo", &message)
            .await;
        histogram
            .record(start.elapsed().as_micros() as u64)
            .unwrap();
    }

    histogram
}

/// スループット測定用の関数
async fn measure_throughput(
    channel: &UnisonChannel,
    message_size: usize,
    duration_secs: u64,
) -> f64 {
    let start = std::time::Instant::now();
    let mut count = 0u64;

    while start.elapsed().as_secs() < duration_secs {
        let message = json!({
            "data": "x".repeat(message_size),
            "sequence": count
        });

        if channel
            .request::<_, serde_json::Value>("echo", &message)
            .await
            .is_ok()
        {
            count += 1;
        }
    }

    count as f64 / start.elapsed().as_secs_f64()
}

/// サーバーのセットアップ
async fn setup_server() -> Arc<Barrier> {
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();

    tokio::spawn(async move {
        let server = ProtocolServer::new();

        // Echo チャネルハンドラー
        server
            .register_channel("bench", |_ctx, stream| async move {
                let channel: UnisonChannel = UnisonChannel::new(stream);
                loop {
                    let msg = match channel.recv().await {
                        Ok(msg) => msg,
                        Err(_) => break,
                    };
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

        let _ = server.listen("[::1]:0").await;

        barrier_clone.wait().await;

        // サーバーを維持
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });

    // サーバーの起動を待つ
    tokio::time::sleep(Duration::from_millis(100)).await;

    barrier
}

fn bench_latency(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("quic_latency");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for &size in MESSAGE_SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.to_async(&runtime).iter(|| async move {
                let barrier = setup_server().await;
                let quic_client = QuicClient::new().unwrap();
                let client = ProtocolClient::new(quic_client);
                client.connect("[::1]:8080").await.unwrap();

                let channel = client.open_channel("bench").await.unwrap();
                let histogram = measure_latency(&channel, size, 100).await;

                channel.close().await.unwrap();
                barrier.wait().await;

                black_box(histogram.mean())
            });
        });
    }

    group.finish();
}

fn bench_throughput(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("quic_throughput");
    group.measurement_time(Duration::from_secs(20));
    group.sample_size(10);

    for &size in MESSAGE_SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.to_async(&runtime).iter(|| async move {
                let barrier = setup_server().await;
                let quic_client = QuicClient::new().unwrap();
                let client = ProtocolClient::new(quic_client);
                client.connect("[::1]:8080").await.unwrap();

                let channel = client.open_channel("bench").await.unwrap();
                let throughput = measure_throughput(&channel, size, 5).await;

                channel.close().await.unwrap();
                barrier.wait().await;

                black_box(throughput)
            });
        });
    }

    group.finish();
}

fn bench_connection_establishment(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    c.bench_function("quic_connection_establishment", |b| {
        b.to_async(&runtime).iter(|| async {
            let barrier = setup_server().await;

            let start = std::time::Instant::now();
            let quic_client = QuicClient::new().unwrap();
            let client = ProtocolClient::new(quic_client);
            client.connect("[::1]:8080").await.unwrap();
            let elapsed = start.elapsed();

            client.disconnect().await.unwrap();
            barrier.wait().await;

            black_box(elapsed)
        });
    });
}

fn bench_concurrent_connections(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("quic_concurrent_connections");

    for &num_clients in &[1, 5, 10, 20, 50] {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_clients),
            &num_clients,
            |b, &num_clients| {
                b.to_async(&runtime).iter(|| async move {
                    let barrier = setup_server().await;
                    let client_barrier = Arc::new(Barrier::new(num_clients + 1));

                    let mut handles = vec![];

                    for _ in 0..num_clients {
                        let client_barrier_clone = client_barrier.clone();
                        let handle = tokio::spawn(async move {
                            let quic_client = QuicClient::new().unwrap();
                            let client = ProtocolClient::new(quic_client);
                            client.connect("[::1]:8080").await.unwrap();

                            let channel = client.open_channel("bench").await.unwrap();

                            // 全クライアントが接続するまで待つ
                            client_barrier_clone.wait().await;

                            // 100回のリクエストを送信
                            for i in 0..100 {
                                let _: Result<serde_json::Value, _> = channel
                                    .request(
                                        "echo",
                                        &json!({
                                            "data": "test",
                                            "sequence": i
                                        }),
                                    )
                                    .await;
                            }

                            channel.close().await.unwrap();
                            client.disconnect().await.unwrap();
                        });
                        handles.push(handle);
                    }

                    // 全クライアントを開始
                    client_barrier.wait().await;

                    // 全クライアントの完了を待つ
                    for handle in handles {
                        handle.await.unwrap();
                    }

                    barrier.wait().await;
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_latency,
    bench_throughput,
    bench_connection_establishment,
    bench_concurrent_connections
);

criterion_main!(benches);
