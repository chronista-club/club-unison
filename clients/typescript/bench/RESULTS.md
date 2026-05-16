# TS Client SDK — Performance Baseline

**v1.0.0-alpha.2 baseline**

> **構造化ログ**: 数値データは KDL で `bench/runs/<version>.kdl` に immutable 記録。
> `bench/index.kdl` が append-only の run インデックス。 `bench/viewer.html` を
> 静的サーバ経由で開くと run 横断の比較表（`cd clients/typescript/bench && npx serve`）。
> 本 md は alpha.2 baseline の詳細分析（= 人間向け companion）。

| 項目 | 値 |
|------|-----|
| 計測日 | 2026-05-16 |
| マシン | Apple Silicon Mac (`uname -m` = `arm64`) |
| Node | v22.22.2 |
| ランナー | vitest 3 `bench`（warmup 有効、cold-start ノイズ除外） |
| 対象 | `@chronista-club/unison-client` v1.0.0-alpha.2 |

実行: `npm run bench`（= `vitest bench`）。`hz` = ops/sec（高いほど速い）、`rme` = 相対平均誤差。

> **正直な計測方針**: チェリーピックも数字の盛りもしない。下記は実出力そのまま。
> run-to-run の振れも観測しており（後述）、絶対値は ±数% の幅で読むこと。

---

## 1. codec.bench.ts — JsonCodec / ProtoCodec encode + decode

`JsonCodec` と `ProtoCodec` の encode/decode を small（~3 フィールド）/ medium
（~12 フィールド）/ large（~100 オブジェクトの配列）で計測。`ProtoCodec` は
proto descriptor codegen が未実装のため、well-known type `google.protobuf.Struct`
を schema fixture として使用（同一 payload を JSON/proto 双方で測れて公平）。
`MessageShape` 値は bench loop 外で `fromJson` 経由で 1 度だけ構築し、純粋な
wire 変換コストのみを計測対象にしている。

### JsonCodec.encode

| payload | hz | mean (ms) | p99 (ms) | rme |
|---------|-----|-----------|----------|-----|
| small  | 2,513,720 | 0.0004 | 0.0006 | ±0.56% |
| medium | 1,263,520 | 0.0008 | 0.0012 | ±0.51% |
| large  |    39,895 | 0.0251 | 0.0559 | ±0.57% |

### JsonCodec.decode

| payload | hz | mean (ms) | p99 (ms) | rme |
|---------|-----|-----------|----------|-----|
| small  | 2,706,584 | 0.0004 | 0.0005 | ±0.12% |
| medium |   937,847 | 0.0011 | 0.0012 | ±0.62% |
| large  |    30,152 | 0.0332 | 0.0434 | ±0.31% |

### ProtoCodec.encode

| payload | hz | mean (ms) | p99 (ms) | rme |
|---------|-----|-----------|----------|-----|
| small  | 273,862 | 0.0037 | 0.0048 | ±1.06% |
| medium |  53,489 | 0.0187 | 0.0257 | ±0.83% |
| large  |   1,522 | 0.6569 | 0.9046 | ±0.93% |

### ProtoCodec.decode

| payload | hz | mean (ms) | p99 (ms) | rme |
|---------|-----|-----------|----------|-----|
| small  | 477,526 | 0.0021 | 0.0027 | ±0.80% |
| medium |  90,268 | 0.0111 | 0.0157 | ±0.79% |
| large  |   2,723 | 0.3672 | 0.6430 | ±1.02% |

### 観察（codec）

- **JSON が proto より速い、すべてのサイズで。** small encode で JSON は proto の
  約 9 倍（2.51M vs 0.27M hz）、large encode で約 26 倍（39.9k vs 1.5k hz）。
  V8 の `JSON.stringify`/`parse` は高度に最適化されたネイティブ実装であるのに対し、
  `@bufbuild/protobuf` の `toBinary`/`fromBinary` は descriptor を辿る JS 実装。
  TS クライアントの単一プロセス内コストとしては JSON が有利、という素直な結果。
  proto の利点は wire サイズと Rust server 側との互換であり、TS 側 CPU では出ない。
- **large での落ち込みが激しい。** proto encode は large が small の約 180 倍遅く、
  絶対値で 1 op あたり 0.66ms。100 要素配列を Struct で表現すると `Value` の
  oneof ラッピングがネストして要素ごとにオーバーヘッドが乗るのが効いていると思われる。
  実運用で 100 要素級の proto batch を hot path に置くのは要注意。
- ProtoCodec の large は本来の codegen された具体 message schema より遅い可能性が高い
  （Struct は汎用表現で最も非効率なケース）。codegen 実装後に再計測すべき。

---

## 2. channel.bench.ts — UnisonChannel.request / DatagramChannel 配送

in-memory mock transport（`tests/integration/mock_transport.ts`）上で計測。
transport は memory pipe のため I/O コストはほぼゼロ。計測対象は SDK 側の
frame encode/decode + codec + recv loop dispatch + Promise/AsyncQueue である。

| bench | hz | mean (ms) | p99 (ms) | rme |
|-------|-----|-----------|----------|-----|
| `UnisonChannel.request()` round-trip（echo, ~6 フィールド payload） | 123,723 | 0.0081 | 0.0300 | ±0.89% |
| `DatagramChannel` event delivery（send→demux→decode→deliver 1 件） | 882,981 | 0.0011 | 0.0017 | ±0.74% |

### 観察（channel）

- request round-trip は 1 op あたり約 8μs。frame encode（length prefix + JSON
  header）→ codec encode → mock pipe → server stub の decode/encode → 戻り
  → recv loop の decode、までを含む。memory transport 上の SDK オーバーヘッドの
  下限値とみなせる。実 WebTransport 上では QUIC RTT が支配的になる。
- datagram delivery が request の約 7 倍速いのは、length-prefixed frame の
  header JSON encode/decode が無く、bidi stream の往復もないため。妥当。
- **p99/p999 の裾が広い**: request の p99=0.030ms に対し p999=0.151ms（mean の
  約 19 倍）。`setTimeout` ベースの request timeout タイマー設定と GC によるもの
  と推測。mean は安定だが tail latency は GC 影響を受ける、と読むべき。

---

## 3. datagram.bench.ts — varint / dispatcher demux

### varint encode / decode（`src/channel/varint.ts`）

| bench | hz | mean (ms) | rme |
|-------|-----|-----------|-----|
| encode 1-byte（< 128） | 8,909,827 | 0.0001 | ±0.79% |
| encode 3-byte          | 8,600,269 | 0.0001 | ±2.48% |
| decode 1-byte          | 21,666,539 | 0.00005 | ±0.34% |
| decode 3-byte          | 20,907,240 | 0.00005 | ±0.51% |

### dispatcher demux + fan-out（`src/channel/dispatcher.ts`）

| bench | hz | mean (ms) | p99 (ms) | rme |
|-------|-----|-----------|----------|-----|
| 256 datagrams across 8 channels（1 op = 256 dispatch） | 73,641 | 0.0136 | 0.0172 | ±2.26% |

### 観察（datagram）

- varint decode が encode より速い（約 21M vs 9M hz）。encode は `number[]` を
  作って `Uint8Array.from` するアロケーションがあるのに対し、decode は確保ゼロの
  純粋ループ。encode 側にアロケーション削減の余地あり（ただし絶対値は十分速い）。
- dispatcher demux は 1 datagram あたり約 53ns（13.6μs ÷ 256）。varint decode +
  Map lookup + `subarray` のコスト。256 件一括で 13.6μs なので fan-out は十分軽い。
- **dispatcher bench の rme=±2.26% はこのスイートで最も高い。** 1 op が 256 回の
  内部ループで GC タイミングを跨ぎやすいため。絶対値は信頼できるが、±2〜3% の
  振れ込みで読むこと。encode 3-byte の rme=±2.48% も同様の理由。

---

## 全体所感（正直ベース）

- 大半の bench は rme < 1% で安定。例外は dispatcher demux と varint encode
  3-byte（±2〜3%）。これらは「1 op に複数回の内部処理 / アロケーションを含み
  GC を跨ぎやすい」ことが原因で、計測ミスではなく実挙動の振れ。
- run-to-run でも数字は動く。例えば `JsonCodec.encode` small は別 run で
  2.51M〜2.66M hz の幅で観測。**baseline 値は ±数% の幅で読むべき**であり、
  将来の回帰判定では 5% 程度の差はノイズとみなすのが妥当。
- 想定外だった点: **TS 側 CPU では JSON が proto を一貫して上回る**。proto の
  large encode は JSON large の約 26 倍遅い。proto を選ぶ理由は wire サイズと
  Rust 互換であって TS の encode 速度ではない、と明確化された。
- バグは発見されなかった。`src/` は未変更。
- ProtoCodec の数字は Struct fixture（汎用表現で最も非効率）由来のため、
  KDL→proto-descriptor codegen 実装後に具体 message schema で再計測が必要。
