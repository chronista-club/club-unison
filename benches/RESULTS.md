# Unison Protocol Benchmark Baseline (Living Doc)

> **Status**: 初期 baseline (v0.9.0、 2026-05-15)
> **Purpose**: 設計指針として継続更新する **living doc**。 各 release で再測定 → diff を「速くなった / 遅くなった」 の判断材料に使う。
> **Update policy**: 各 minor/major release で `cargo bench --workspace` を実走、 結果を本ファイルに上書き (= overwrite で履歴は git history に残す)。 future で CI に組み込み (team-b dispatch)。

---

## 計測環境

| 項目 | 値 |
|------|----|
| 計測日 | 2026-05-15 |
| 計測 host | macOS / arm64 (= Mac M-series) |
| Rust toolchain | 1.95.0 stable |
| Build profile | release |
| RUSTFLAGS | `-C symbol-mangling-version=v0` (macOS 必須) |
| criterion | 0.8 |
| 計測 duration | warmup 1s / measurement 5s (= short pass、 CI 友好) |

---

## bench: `throughput` (= `crates/unison-protocol/benches/throughput.rs`)

メッセージ処理の **request/response / streaming / parallel / burst** スループットを各種パラメータで測定。 計測は **measurement-time 5s** で実施 (= criterion default 100 samples、 short pass)、 数値は **medium-term 信頼区間 ±**。

### `message_throughput` (request/response 往復、 ペイロード × バッチ)

| Payload | Batch 100 | Batch 1000 | 備考 |
|---------|-----------|------------|------|
| 8 KB | **122 ms ± 5.5 ms** / iter | **189 ms ± 10.9 ms** / iter | 100→1000 で 1.55× (sub-linear、 batch amortize 効きあり) |

> 注: `Payload < 8 KB` および `Batch < 100` は short measurement 内で 100 samples 未達の警告で埋もれた。 measurement-time 30s+ で再測定する場合は `--measurement-time 30 --sample-size 20` 推奨。

### `streaming_throughput` (固定 iter で stream stream send)

| Stream size | ns / iter | 備考 |
|-------------|-----------|------|
| 128 B | **1.103 s ± 0.8 ms** | size に依存せず一定 |
| 512 B | **1.104 s ± 4.9 ms** | |
| 2 KB | **1.103 s ± 7.8 ms** | |
| 8 KB | **1.103 s ± 0.5 ms** | |

→ stream は **payload size に依存せず ~1.1 s / iter で flat**。 つまり bench 内 fixed iteration loop が 1.1 s で完了 (= round trip 数支配)、 payload は無視可能。 設計仮説「**stream channel は HoL blocking 許容、 payload size 二次的**」 と整合。

### `parallel_throughput` (worker 数 vs total time)

| Workers | ns / iter | vs baseline | 観察 |
|---------|-----------|-------------|------|
| 1 | **1.103 s ± 4.3 ms** | 1.00× | baseline |
| 2 | 1.104 s ± 3.2 ms | 1.001× | ほぼ同 |
| 4 | 1.105 s ± 0.7 ms | 1.001× | ほぼ同 |
| 8 | 1.107 s ± 10.9 ms | 1.004× | わずか overhead |
| 16 | 1.123 s ± 24.9 ms | **1.018×** | overhead 顕在 |

→ **parallel scaling flat** (= worker 1→16 で total time +1.8% のみ)。 単一 channel 内では QUIC 多重化の恩恵が薄く、 lock / mutex / await 順序が支配的の可能性。 **設計指針**: 性能が必要なら **チャネル間並列化** (= 複数 channel を別 task で動かす) が筋、 同 channel 内 worker 数増は ROI 低い。

### `burst_throughput` (burst size linear)

| Burst size | ns / iter | vs burst 10 | 観察 |
|-----------|-----------|-------------|------|
| 10 | **106 ms ± 1.1 ms** | 1.00× | baseline |
| 50 | 110 ms ± 4.9 ms | 1.05× | |
| 100 | 115 ms ± 34.6 ms | 1.09× | 分散大 |
| 500 | 142 ms ± 21.3 ms | 1.35× | |
| 1000 | 163 ms ± 16.5 ms | **1.55×** | sub-linear、 burst amortize 効く |

→ burst が大きいほど **1 message あたり overhead が下がる** (= sub-linear)。 100 → 1000 で 1.42× (10× burst で latency 1.42×)、 amortization 効いている。 設計仮説「**batch / burst は重要 path で性能改善**」 と整合。

---

## bench: `quic_performance` (= `crates/unison-protocol/benches/quic_performance.rs`)

QUIC 接続 + channel open + 連続 request の **接続単位パフォーマンス** を hdrhistogram で latency 分布まで含めて測定。 v0.9.0 では throughput.rs を先行 measure、 quic_performance は **measurement-time 10-20s** が必要 (= bench 内設定)、 v0.10+ で別 measurement で取得予定 (= team-b dispatch / release CI 自動化と組み合わせ)。

### 計測予定

- `quic_latency/{64, 256, 1024, 4096, 16384}` (5 message size の p50/p95/p99 latency)
- `quic_throughput/{64, 256, 1024, 4096, 16384}` (5 message size の sustained throughput)
- `quic_connection_establishment` (single connection 確立時間)
- `quic_concurrent_connections/{1, 5, 10, 20, 50}` (並行 client 数 vs 接続オーバーヘッド)

---

## 注意事項

- 本数値は **1 ホスト 1 回の計測**、 統計的有意性は弱い (criterion の 5 sec measurement は CI 友好の short pass)。 production レイテンシの参考にする際は本格 measurement (= measurement-time 30s+) を取り直すこと。
- packet 層は rkyv 0.7 archive (= zero-copy)、 v0.10+ で wire format pluggable 化 (`design/wire-format.md` 参照) 後は format 別の比較も追加予定。

---

## 過去 baseline (= release 越しの傾向)

| Release | 計測日 | message 8KB/1000 batch | streaming 8KB | parallel 16 workers | burst 1000 | Notes |
|---------|--------|------------------------|---------------|---------------------|------------|-------|
| v0.9.0 | 2026-05-15 | 189 ms / iter | 1.103 s / iter (flat) | 1.123 s / iter (+1.8% vs 1 worker) | 163 ms / iter (1.55× vs burst 10) | 初期 baseline、 「基盤整備」 release。 計測 host: macOS arm64、 measurement-time 5s short pass |

---

## v0.9.0 主要観察

3 つの設計指針が baseline から見えた:

1. **stream は payload に依存せず flat**: stream throughput は 128B → 8KB で variance 5%。 「stream は HoL blocking 許容、 payload size 二次的」 という spec/02 設計と整合。 stream で大量 byte を送る path は payload size 上げて round trip 減らすのが有利。
2. **同一 channel 内 parallel scaling は flat**: worker 1 → 16 で +1.8% のみ。 性能が必要なら **チャネル間並列化** (= 別 task で別 channel) が筋論、 同 channel 内 worker 増は ROI 低い。 v0.10+ の channel 多重化最適化の design 入り口。
3. **batch / burst amortization は効く**: burst 10 → 1000 で 1.55× のみ (10× burst で latency 1.55×)、 sub-linear で per-message overhead が逓減。 設計上は **「batch を大きく取れる path」** を encourage する形にすべき。
