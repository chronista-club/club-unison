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

メッセージ処理の **request/response 往復スループット** を payload size × batch size の 4 × 4 = 16 ケースで測定。

| Payload size | Batch 1 | Batch 10 | Batch 100 | Batch 1000 |
|--------------|---------|----------|-----------|------------|
| 128 B | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| 512 B | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| 2 KB | _TBD_ | _TBD_ | _TBD_ | _TBD_ |
| 8 KB | _TBD_ | _TBD_ | _TBD_ | _TBD_ |

(数値は 1 秒あたり処理 element 数 / レイテンシは別 col 検討)

### 観察 / 設計仮説

- (= bench 結果出たら埋める)

---

## bench: `quic_performance` (= `crates/unison-protocol/benches/quic_performance.rs`)

QUIC 接続 + channel open + 連続 request の **接続単位パフォーマンス** を測定。 hdrhistogram で latency 分布も取得。

### 結果

(= bench 結果出たら埋める、 p50/p95/p99 + max latency など)

---

## 注意事項

- 本数値は **1 ホスト 1 回の計測**、 統計的有意性は弱い (criterion の 5 sec measurement は CI 友好の short pass)。 production レイテンシの参考にする際は本格 measurement (= measurement-time 30s+) を取り直すこと。
- packet 層は rkyv 0.7 archive (= zero-copy)、 v0.10+ で wire format pluggable 化 (`design/wire-format.md` 参照) 後は format 別の比較も追加予定。

---

## 過去 baseline (= release 越しの傾向)

| Release | 計測日 | Throughput median (8KB / batch 1000) | QUIC p99 latency | Notes |
|---------|--------|--------------------------------------|------------------|-------|
| v0.9.0 | 2026-05-15 | _TBD_ | _TBD_ | 初期 baseline、 「基盤整備」 release |
