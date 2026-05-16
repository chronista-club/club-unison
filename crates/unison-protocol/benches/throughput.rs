use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;
use unison::network::channel::UnisonChannel;
use unison::network::{MessageType, quic::QuicClient};
use unison::{ProtocolClient, ProtocolServer};

/// バッチサイズのバリエーション
const BATCH_SIZES: &[u64] = &[1, 10, 100, 1000];

/// メッセージペイロードサイズ
const PAYLOAD_SIZES: &[usize] = &[128, 512, 2048, 8192];

/// エコーチャネルハンドラーを登録するヘルパー
async fn register_echo_channel(server: &ProtocolServer, counter: Arc<AtomicU64>) {
    server
        .register_channel("bench", move |_ctx, stream| {
            let counter = counter.clone();
            async move {
                let channel: UnisonChannel = UnisonChannel::new(stream);
                loop {
                    let msg = match channel.recv().await {
                        Ok(msg) => msg,
                        Err(_) => break,
                    };
                    if msg.msg_type == MessageType::Request {
                        counter.fetch_add(1, Ordering::Relaxed);
                        let payload = msg.payload_as_value().unwrap_or_default();
                        let response = json!({
                            "status": "processed",
                            "id": payload.get("id").cloned().unwrap_or(json!(0))
                        });
                        if channel
                            .send_response(msg.id, &msg.method, &response)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Ok(())
            }
        })
        .await;
}

/// メッセージ処理のスループット測定
fn bench_message_throughput(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("message_throughput");

    for &payload_size in PAYLOAD_SIZES {
        for &batch_size in BATCH_SIZES {
            let bench_name = format!("payload_{}_batch_{}", payload_size, batch_size);

            group.throughput(Throughput::Elements(batch_size));
            group.bench_function(bench_name, |b| {
                b.to_async(&runtime).iter(|| async move {
                    let processed = Arc::new(AtomicU64::new(0));

                    tokio::spawn({
                        let processed = processed.clone();
                        async move {
                            let server = ProtocolServer::new();
                            register_echo_channel(&server, processed).await;
                            let _ = server.listen("[::1]:8081").await;
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                        }
                    });

                    tokio::time::sleep(Duration::from_millis(100)).await;

                    let quic_client = QuicClient::new().unwrap();
                    let client = ProtocolClient::new(quic_client);
                    client.connect("[::1]:8081").await.unwrap();

                    let channel = client.open_channel("bench").await.unwrap();
                    let payload_data = "x".repeat(payload_size);

                    for i in 0..batch_size {
                        let _: Result<serde_json::Value, _> = channel
                            .request(
                                "process",
                                &json!({
                                    "id": i,
                                    "data": payload_data.clone()
                                }),
                            )
                            .await;
                    }

                    channel.close().await.unwrap();
                    client.disconnect().await.unwrap();

                    black_box(processed.load(Ordering::Relaxed))
                });
            });
        }
    }

    group.finish();
}

/// ストリーミングスループット測定
fn bench_streaming_throughput(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("streaming_throughput");
    group.measurement_time(Duration::from_secs(10));

    for &payload_size in PAYLOAD_SIZES {
        group.throughput(Throughput::Bytes(payload_size as u64));
        group.bench_function(format!("stream_{}_bytes", payload_size), |b| {
            b.to_async(&runtime).iter(|| async move {
                tokio::spawn(async move {
                    let server = ProtocolServer::new();
                    register_echo_channel(&server, Arc::new(AtomicU64::new(0))).await;
                    let _ = server.listen("[::1]:8082").await;
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                });

                tokio::time::sleep(Duration::from_millis(100)).await;

                let quic_client = QuicClient::new().unwrap();
                let client = ProtocolClient::new(quic_client);
                client.connect("[::1]:8082").await.unwrap();

                let channel = client.open_channel("bench").await.unwrap();
                let payload_data = "x".repeat(payload_size);
                let start = std::time::Instant::now();
                let mut bytes_sent = 0u64;

                while start.elapsed() < Duration::from_secs(1) {
                    if channel
                        .request::<_, serde_json::Value>(
                            "stream",
                            &json!({"data": payload_data.clone()}),
                        )
                        .await
                        .is_ok()
                    {
                        bytes_sent += payload_size as u64;
                    }
                }

                channel.close().await.unwrap();
                client.disconnect().await.unwrap();

                black_box(bytes_sent)
            });
        });
    }

    group.finish();
}

/// 並列処理のスループット測定
fn bench_parallel_throughput(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("parallel_throughput");
    group.measurement_time(Duration::from_secs(15));

    for &num_workers in &[1, 2, 4, 8, 16] {
        group.bench_function(format!("workers_{}", num_workers), |b| {
            b.to_async(&runtime).iter(|| async move {
                let counter = Arc::new(AtomicU64::new(0));

                tokio::spawn({
                    let counter = counter.clone();
                    async move {
                        let server = ProtocolServer::new();
                        register_echo_channel(&server, counter).await;
                        let _ = server.listen("[::1]:8083").await;
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                });

                tokio::time::sleep(Duration::from_millis(100)).await;

                let mut handles = vec![];
                for _ in 0..num_workers {
                    let handle = tokio::spawn(async move {
                        let quic_client = QuicClient::new().unwrap();
                        let client = ProtocolClient::new(quic_client);
                        client.connect("[::1]:8083").await.unwrap();

                        let channel = client.open_channel("bench").await.unwrap();
                        let mut local_count = 0u64;
                        let start = std::time::Instant::now();

                        while start.elapsed() < Duration::from_secs(1) {
                            if channel
                                .request::<_, serde_json::Value>("work", &json!({}))
                                .await
                                .is_ok()
                            {
                                local_count += 1;
                            }
                        }

                        channel.close().await.unwrap();
                        client.disconnect().await.unwrap();
                        local_count
                    });
                    handles.push(handle);
                }

                let mut total = 0u64;
                for handle in handles {
                    total += handle.await.unwrap();
                }

                black_box(total)
            });
        });
    }

    group.finish();
}

/// バースト処理のスループット測定
fn bench_burst_throughput(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    let mut group = c.benchmark_group("burst_throughput");

    for &burst_size in &[10, 50, 100, 500, 1000] {
        group.throughput(Throughput::Elements(burst_size));
        group.bench_function(format!("burst_{}", burst_size), |b| {
            b.to_async(&runtime).iter(|| async move {
                tokio::spawn(async move {
                    let server = ProtocolServer::new();
                    register_echo_channel(&server, Arc::new(AtomicU64::new(0))).await;
                    let _ = server.listen("[::1]:8084").await;
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                });

                tokio::time::sleep(Duration::from_millis(100)).await;

                let quic_client = QuicClient::new().unwrap();
                let client = ProtocolClient::new(quic_client);
                client.connect("[::1]:8084").await.unwrap();

                let channel = client.open_channel("bench").await.unwrap();
                let start = std::time::Instant::now();
                let mut success_count = 0;

                for i in 0..burst_size {
                    let result: Result<serde_json::Value, _> = channel
                        .request(
                            "burst",
                            &json!({
                                "id": i,
                                "timestamp": chrono::Utc::now().to_rfc3339()
                            }),
                        )
                        .await;
                    if result.is_ok() {
                        success_count += 1;
                    }
                }

                let elapsed = start.elapsed();

                channel.close().await.unwrap();
                client.disconnect().await.unwrap();

                black_box((success_count, elapsed))
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_message_throughput,
    bench_streaming_throughput,
    bench_parallel_throughput,
    bench_burst_throughput
);

criterion_main!(benches);
