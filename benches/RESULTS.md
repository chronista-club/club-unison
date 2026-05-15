# Unison Protocol Benchmark Baseline (Living Doc)

> **Status**: v0.10.1 fresh baseline (= 2026-05-16 から新規開始)
> **Purpose**: 設計指針として継続更新する **living doc**。 各 release で再測定 → overwrite で記録、 履歴は git history。
> **Note**: v0.9.0 baseline (= 2026-05-15、 cold-start per-iter semantic) は **本 file 履歴から除外**。 v0.10.0 で buffa pivot + datagram channel API 内部実装が大きく変わり、 v0.10.1 で bench code 自体も rewrite (= steady-state semantic、 OS-assigned port、 shared connection per iter) したため、 過去数字との直接比較は misleading。 **v0.10.1 を新 baseline として今後の patch / minor で diff を計測**。

---

## 計測環境

| 項目 | 値 |
|------|----|
| 計測日 | 2026-05-16 |
| 計測 host | macOS / arm64 (= Mac M-series) |
| Rust toolchain | 1.95.0 stable |
| Build profile | release |
| RUSTFLAGS | `-C symbol-mangling-version=v0` (macOS 必須) |
| criterion | 0.8 |
| Bench semantic | **steady-state** (= 1 connection × N iter で共有、 setup overhead exclude)。 cold-start per-iter は ephemeral port / fd 枯渇で macOS で詰まる、 honest data を取るため shape を切替。 |

---

## bench: `datagram` — raw connection-level datagram (= MVP API)

`crates/unison-protocol/benches/datagram.rs` — `QuicClient::send_datagram` / `recv_datagram` の **connection-level raw** API を測定 (= channel API 抽象を bypass、 caller が demux header を payload に含める path)。 server 側は raw quinn endpoint で datagram echo。

### Bench shape

- Setup: 1 server endpoint + 1 QuicClient connect (= per bench case 1 回、 iter で共有)
- iter: burst N (= payload bytes を burst_count 連射) → echo recv until count or timeout
- Metric: 1 burst-recv round trip time / iter

### Results (= median time per burst-recv cycle)

| Payload | Burst | Time / iter (median) | 95%ile bounds |
|---|---|---|---|
| 64 B | 100 | **127.23 µs** | 127.23 / 133.31 / 146.82 µs |
| 64 B | 1000 | **665.42 µs** | 665.42 / 680.12 / 701.73 µs |
| 1300 B | 100 | **31.93 ms** | 31.93 / 37.51 / 51.27 ms |
| 1300 B | 1000 | **506.98 ms** | 506.98 / 509.58 / 512.04 ms |

### 観察

- **64B × 100 = 127µs**: 1 transform / burst の baseline、 1 datagram あたり ~1.27 µs (= QUIC send/recv overhead 込み)
- **64B × 1000 = 665µs**: burst 増えても 1 datagram あたり ~0.67 µs に **薄まる** (= 連射 hot path で UDP socket batching が効く)
- **1300B での latency 急増** (= 64B 比 ~250x): MTU 上限 payload は wire 帯域に律速、 100 datagram × 1300B ≈ 130 KB / 32ms ≈ **4 GB/s** 相当 throughput
- **1300B × 1000 = 507ms**: 1.3 MB / 507ms ≈ **2.5 GB/s** sustained、 burst 1000 の interval / scheduling overhead 含む

---

## bench: `datagram_channel` — channel API 経由 burst (= v0.10.0 新)

`crates/unison-protocol/benches/datagram_channel.rs` — `ProtocolServer::register_channel_datagram` + `ProtocolClient::open_datagram_channel` の **channel API path** を測定。 raw bench と同じ payload × burst で比較。

### Bench shape

- Setup: 1 ProtocolServer + handler register + spawn_listen_shared + 1 ProtocolClient + open_datagram_channel (= per bench case 1 回、 iter で共有)
- iter: burst N → echo recv until count or timeout
- payload: `Payload { data: Vec<u8> }`、 JSON codec で wire encoding

### Results

| Payload (input) | Burst | Time / iter (median) | drop / 備考 |
|---|---|---|---|
| 64 B | 100 | **620.51 µs** | drop なし、 raw 比 **4.7x** (= JSON encode + varint demux + dispatcher overhead) |
| 64 B | 1000 | **512.10 ms** | ⚠️ **多数 drop**、 recv timeout 500ms に貼り付き |
| 1300 B | 100 | **504.03 ms** | ⚠️ **全 drop**、 JSON で wire ~5200B (= MTU 超過、 `SendDatagramError::TooLarge`) |
| 1300 B | 1000 | **517.31 ms** | ⚠️ 同上 |

### 観察 — JSON codec の MTU 制約

**重大な発見**: `Payload { data: Vec<u8> }` を JSON encode すると raw bytes が `[171,171,...,171]` の数値配列に展開、 ~4x 拡大。

- input 1300 B → wire ~5200 B = **MTU 1300 超過、 全 datagram drop**
- input 64 B × burst 1000 = 100KB 程度、 ただし高頻度発射で **client 側 datagram send buffer / server 側 dispatcher mpsc が飽和、 drop 発生**

**caller 向け推奨**:
- JSON codec で datagram channel を使う場合の **effective payload limit ≈ 200-300 B input**
- 高頻度 sustained streaming は **ProtoCodec (= buffa-encoded、 wire compact)** を推奨 (v0.10.1 時点では `open_datagram_channel_with::<ProtoCodec>` で選択可)
- 大 payload (= ≥1KB) を datagram で送るのは **anti-pattern**、 stream channel を使うべき

### Channel API overhead 数値化

64B × 100 burst の比較:
- **raw**: 127.23 µs
- **channel**: 620.51 µs
- **diff**: ~493 µs = **3.9x overhead**

内訳推定 (= 100 datagram あたり):
- JSON encode/decode (= 64B → ~280B): ~4.9 µs / datagram = **490 µs** (= 大半)
- varint channel_id encode/decode: ~0.01 µs / datagram (= 無視できる)
- mpsc dispatcher routing: ~0.001 µs / datagram (= 無視できる)

**結論**: channel API overhead は **JSON codec encoding cost が支配**。 ProtoCodec に切替えれば 95% を回避できる見込み (= 別 bench で検証必要、 v0.10.x で追加候補)。

---

## bench: `datagram_channel_sustained` — sustained streaming (= 位置同期 use case、 v0.10.1 新)

`crates/unison-protocol/benches/datagram_channel_sustained.rs` — **realistic position sync** use case (= 60Hz / 120Hz × 数秒 連続 stream) を測定。 burst bench とは別の shape、 「現実的 1 peer position sync の継続安定動作」。

### Bench shape

- Setup: 1 server + 1 channel handler + 1 client + open_datagram_channel + Arc<DatagramChannel> 化 (= send / recv 別 task)
- iter: 1 session = 2 sec stream
  - send task: `tokio::time::interval(1/rate_hz)` で Transform を rate-limited 送信
  - recv task: `recv_event::<Transform>()` を `RECV_POLL_TIMEOUT 20ms` で polling、 session_deadline + 300ms 余裕まで recv
- payload: `Transform { id, pos: [f32;3], rot: [f32;4] }`、 JSON wire ~110-130 B (= MTU 内)

### Results

| Rate | Stream duration | Session time | sent / iter | recv / iter | Drop rate |
|---|---|---|---|---|---|
| 60 Hz | 2 sec | **2.32 s** | 121 | 121 | **0.0%** |
| 120 Hz | 2 sec | **2.32 s** | 241 | 241 | **0.0%** |

### 観察

- **drop 0%** at 60Hz / 120Hz: v0.10.0 datagram channel API は realistic position sync use case (= single peer × refresh rate) で **fully reliable steady-state** に到達
- **sent = expected_rate × duration + 1**: tokio::time::interval の first tick が即発火 (= `MissedTickBehavior::Burst` default)、 caller は rate expectation 計算で +1 を考慮
- **session_time ≈ 2.31 s** = STREAM_DURATION 2.0s + RECV_TAIL_BUFFER 0.3s + ~10ms overhead
- **Transform JSON wire** 110-130B は MTU 1300 の 10% 以下、 余裕あり (= 10 peer pack ≈ 1100B、 まだ MTU 内に収まる pack 戦略可能)

### 設計上の含意

「single peer × 60/120Hz × realistic payload」 の baseline は問題なし。 **次に検証すべき shape**:

1. **multi-peer broadcast**: server.broadcast を 100 / 1000 client に対して送る (= drop 始まる threshold)
2. **MTU 限界 pack**: 200-byte transform を MTU 内に何個 pack できるか (= ≈ 10 個 / datagram)
3. **ProtoCodec 比較**: 同じ Transform を ProtoCodec で送信、 wire size + drop rate diff
4. **sustained at higher rate**: 240Hz / 480Hz (= VR headset の next-gen target)

これらは v0.11+ の bench expansion 候補。

---

## bench: `datagram_channel_max_throughput` — 上限値計測 (= rate-limit なし、 v0.10.1 新)

`crates/unison-protocol/benches/datagram_channel_max_throughput.rs` — このマシン (= 計測 host) で datagram channel API が出せる **ceiling** を測る。 sustained (= rate-limited) とは別の system 視点の bench。

### Bench shape

- Setup: 1 server + 1 client + 1 channel + Arc<DatagramChannel> (= sustained と同 pattern)
- iter: 1 session = 2 sec
  - send task: **rate 制限なし**、 tight loop で as-fast-as-possible
  - recv task: poll で受信 count
- Metric: sent/s, recv/s, drop %

### Results (= 12 measurements from 2 separate runs、 v0.10.1 で sent/s と recv/s の分母を統一)

両 metric とも **send window (= `STREAM_DURATION × iters` = 2.0s × iters)** を分母に計算 (= `1 - recv/sent` が drop rate と algebraically 一致する形)。

| Metric | Range | Median |
|---|---|---|
| **sent/s** | 472k - 609k | **~530,000 msg/s** |
| **recv/s** | 343k - 531k | **~487,000 msg/s** |
| **drop rate** | 3.2% - 43.4% | **~5.0%** (= 高 variance、 system load sensitive) |
| sent total / 2sec session | 0.94-1.22 million msgs | ~1.05 million |
| session time | 2.31 s | 2.31 s |

各 iter の raw numbers (= 12 sample = 10 measurement + 2 warmup):

```
sent=1109728 recv=1063400 drop=4.2% sent/s=554864 recv/s=531700
sent=1100567 recv=1032163 drop=6.2% sent/s=550284 recv/s=516082
sent=1005608 recv=953012  drop=5.2% sent/s=502804 recv/s=476506
sent=1059863 recv=930916  drop=12.2% sent/s=529932 recv/s=465458
sent=1017081 recv=974421  drop=4.2% sent/s=508540 recv/s=487210
sent=1052940 recv=996407  drop=5.4% sent/s=526470 recv/s=498204
sent=999121  recv=685825  drop=31.4% sent/s=499560 recv/s=342912
sent=945197  recv=874148  drop=7.5% sent/s=472598 recv/s=437074
sent=1218564 recv=689325  drop=43.4% sent/s=609282 recv/s=344662
sent=1074532 recv=833691  drop=22.4% sent/s=537266 recv/s=416846
sent=1026136 recv=974816  drop=5.0% sent/s=513068 recv/s=487408
(+ 1 warmup iter)
```

### 観察 — high variance under saturation

- **median ceiling ~487k recv msg/s** at Mac M-series localhost、 60Hz × 1 peer (60 msg/s) に対し ~8,100x headroom
- **drop variance 3-43%** = saturation 域では **system load によって drop rate が大きく変動**。 background process / GC pause / scheduler 揺らぎが顕在化、 low-drop iter (= 4%) と high-drop iter (= 43%) の差が 10x
- **ceiling は "median" ではなく "range"** で見るべき: realistic deployment では 5-15% drop を見込む、 best case 数字 (= 4%) を採用すると capacity overestimate
- **sustained 120Hz が drop 0%** だったのは ceiling の **0.05% 程度** しか使っていなかったから (= 240 msg/s vs 487k msg/s median ceiling)
- **caller の capacity planning 推奨**:
  - 「100 peer × 60Hz = 6k msg/s = ceiling の 1.2% = drop ~0% 想定」
  - 「1000 peer × 60Hz = 60k msg/s = ceiling の 12% = **drop 3-5% 想定、 caller 側で frame skip 戦略推奨**」
  - 「10000 peer × 60Hz = 600k msg/s = ceiling 超え、 **broadcast を server side でしない (= 各 client が subscribe 戦略)** に切替必要」

### 計測 topology の制約 (= 重要)

本数字は **localhost (= 同 machine、 loopback)** での測定値、 **realistic cloud / network 環境では大きく変わる**:

| Topology | 推定 ceiling (= 本 bench shape) | 推定 latency |
|---|---|---|
| **localhost** (= 計測値) | ~445k msg/s | ~1µs |
| 同 host container (= shared kernel、 docker network) | ~400k msg/s | ~5µs |
| 同 AZ (= 同 data center) | ~50-100k msg/s | ~0.1-1ms RTT |
| Cross-AZ (= 同 region) | ~10-50k msg/s | ~1-5ms RTT |
| Cross-region (= geographic) | ~1-10k msg/s | ~50-200ms RTT |

cloud / WAN 計測は network bandwidth + latency + packet loss が支配的、 localhost 数字を **「software ceiling」** と解釈、 deployment ceiling は別。

### v0.11+ task: cloud bench

caller (= chronista-club ecosystem の Fly.io / Cloud Run 等の realistic deployment) で動かす想定の bench を加える。 候補 design:

1. **docker-compose pattern**: 2 container (= server / client) を `network: bridge` で接続、 同 host container 間で測定
2. **CI integration**: GitHub Actions / Fly.io ephemeral deploy で cross-host 計測、 RESULTS.md に「localhost vs container vs cloud」 の 3 段比較を記録
3. **deployment hint doc**: caller 向けに「自環境で本 bench を回す手順」 を `benches/CLOUD_HOWTO.md` として整備

これは v0.11+ で polyglot client base release theme と一緒に組み込み検討。

---

## bench: `ping_pong` — stream channel request/response baseline

`crates/unison-protocol/benches/ping_pong.rs` — `UnisonChannel::request<Req, Resp>` の round-trip latency baseline (= stream channel、 1 request / 1 response)。 payload 4 ケース。

### Results

| Payload | Time / iter (median) | 95%ile bounds |
|---|---|---|
| 16 B | **155.26 ms** | 155.26 / 155.57 / 155.93 ms |
| 64 B | **155.00 ms** | 155.00 / 155.24 / 155.49 ms |
| 256 B | **154.98 ms** | 154.98 / 155.26 / 155.54 ms |
| 1024 B | **155.20 ms** | 155.20 / 156.75 / 158.74 ms |

### 観察

- **payload size に依存せず ~155 ms / iter で flat**: 1 request/response の latency が **setup + handshake + 1 round trip** に支配される、 payload size (= 16B-1024B 範囲) はほぼ影響しない
- これは v0.9.0 baseline でも同様の傾向、 buffa pivot 後も latency profile は変わらず
- 設計仮説「**stream channel は HoL blocking 許容、 payload size 二次的**」 と整合

---

## bench: `throughput` — request/response / streaming / parallel / burst (= 未測定 in v0.10.1)

`crates/unison-protocol/benches/throughput.rs` — 既存 bench、 v0.10.1 では **bench code 内 固定 port (= 8081-8084)** が macOS 環境で `AddrInUse` 衝突する pre-existing issue で測定 skip。

**v0.11+ task**: throughput.rs を OS-assigned port + steady-state semantic に rewrite。

---

## bench: `quic_performance` — latency / throughput / connection / channel-isolation (= 未測定 in v0.10.1)

`crates/unison-protocol/benches/quic_performance.rs` — 既存 bench、 上記同様 **bench code 内 固定 port (= 8080)** で macOS で衝突。 v0.10.1 では skip。

**v0.11+ task**: 同上、 OS-assigned port rewrite。

---

## v0.11+ bench 拡張候補

1. **cloud / WAN bench** (= 上述、 docker-compose / CI integration / CLOUD_HOWTO.md): localhost ceiling vs container vs cloud の 3 段比較、 realistic deployment 数字の獲得
2. **multi-peer broadcast bench**: `server.broadcast` を 10 / 100 / 1000 client に対して、 drop 始まる threshold を計測
3. **ProtoCodec vs JsonCodec 比較 bench**: 同 Transform payload で codec のみ切替、 wire size + drop rate diff
4. **higher rate sustained**: 240 Hz / 480 Hz position sync (= VR headset 想定)
5. **throughput.rs / quic_performance.rs rewrite**: OS-assigned port + steady-state semantic に統一
6. **bench harness 独自化検討**: criterion の「time per iter」 metric だけでは sustained throughput / drop rate を表現しにくい、 custom bench harness or criterion 拡張 evaluate
7. **CI 上での bench 定期実行 + RESULTS.md auto regen**: team-b dispatch で v0.11+ で自動化検討
