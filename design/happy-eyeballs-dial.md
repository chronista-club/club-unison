# Happy Eyeballs v2 Dial — `connect_race`（staggered-race dialer）

**status**: engine + direct adapter 実装済 2026-07-10（PR #86）/ relay adapter は次段
**動機元**: chronista-hub ADR-020 §S6 の consume 側。到達性 ladder（§D3 `direct → relay`）を *逐次フォールバック* でなく **1 本の時間差レース** に畳む。
**原則**: RFC 8305（Happy Eyeballs v2）の staggered-race を QUIC + relay に一般化。engine は **network 非依存**（data/calc/action の分離）で、実 I/O は薄い adapter に寄せる。

---

## 1. なぜ staggered race か — timeout ジレンマの構造的解消

到達候補が複数あるとき（別アドレスの direct、hub 経由の relay）、素朴な実装は **逐次フォールバック**する: 候補 1 を per-endpoint timeout 付きで試し、ダメなら候補 2 へ。これは 1 本のノブ（timeout）に矛盾した要求を負わせる:

- **短すぎる** → 生きているが遅い経路を「黒穴」と誤判定して捨てる（good-path 誤棄）。
- **長すぎる** → 先頭候補が本当に黒穴のとき、次を試すまで数秒待たされる。

staggered race はこのジレンマを**構造的に消す**。候補を「優先度と stagger（時間差）だけ違うもの」に均し、**全部を時間差で並行 arm** して、最初に握手完了した経路を採用・残りを cancel する。誰も「全滅」を判定しない。唯一の遅延ノブは**有界な stagger** だけで、死経路のコストは stagger 1 tick に収まる（先頭が黒穴でも、次候補は stagger 後に arm 済みで走っている）。

|                        | 逐次フォールバック          | staggered race（本設計）        |
| ---------------------- | --------------------------- | ------------------------------- |
| 死経路のコスト         | per-endpoint timeout（数秒） | **stagger 1 tick（有界）**       |
| good-but-slow 誤棄     | timeout 次第で起きる         | 起きない（待てば採用される）    |
| 「全滅」判定           | 必要（誰かが timeout を持つ） | **不要**（in-flight ゼロで自然決着） |

---

## 2. 到達性 ladder — direct と relay（ADR-020 §D3）

race に乗せる候補は 2 種（[`Candidate`]）:

| candidate            | 到達手段                                             | 全滅時の hub 依存 |
| -------------------- | ---------------------------------------------------- | ----------------- |
| `Direct(SocketAddr)` | その addr へ QUIC 握手（IPv6 GUA /（将来）IPv4 / tailnet） | 無関係（§D3-a、hub data 不在） |
| `Relay(WldId)`       | hub connection 上に `relay` channel を開き `open{to}` 宣言 = **別到達機構**（§D3-b、universal floor） | あり（hub の transient forwarding state） |

`WldId` = location 独立の home-World 番地（ADR-020 D2）。relay 中継の宛先 routing key。

**現状の実装スコープ（direct-first-cut）**: engine は direct/relay 両方を扱えるが、`QuicClient::connect_race` の adapter は **IPv6 GUA direct のみ**配線している。IPv4 は §D3 で deferred（warn して skip、silent drop はしない）。relay fallback は「別到達機構ゆえ `Transport` 抽象が要る」ため次段（§8）。

---

## 3. engine と I/O の分離（§S6 の data / calc / action）

`network::dial` は 3 つに分かれる:

- **`rank(candidates, my_addrs) -> Vec<Candidate>`** — 副作用ゼロの純関数（calc）。hub の opaque 順序付き候補（D2）に *消費側で* 意味を与える。
- **`race<T, F, Fut>(candidates, my_addrs, cfg, attempt) -> Result<Winner<T>, RaceError>`** — network 非依存の generic engine（calc + タイマ）。実際の握手は呼び出し側が渡す `attempt: FnMut(Candidate) -> Fut` closure に閉じる。engine は `Candidate` の direct/relay 区別とタイマだけを見て、I/O を一切知らない。
- **adapter（action）** — `QuicClient::connect_race` が `attempt` に quinn の `endpoint.connect(addr, sni)` を閉じ込める。relay adapter（次段）は hub channel open を閉じ込める。

この分離の**帰結**: 並行タイミングの状態機械を `#[tokio::test(start_paused = true)]` の**仮想時間**で決定論的にテストできる（§7）。全 `attempt` 呼び出しが同一 future 型を返すため `FuturesUnordered<Fut>` で boxing 不要。

### 主要型

```rust
pub enum Candidate { Direct(SocketAddr), Relay(WldId) }

pub struct RaceCfg {
    pub stagger: Duration,          // direct 候補間の時間差（RFC 8305 Connection Attempt Delay）
    pub relay_handicap: Duration,   // 握手済 relay の adoption gate（direct に勝機を与える hold）
    pub overall_deadline: Duration, // 全体の hard deadline
}

pub struct Winner<T> { pub transport: T, pub via: Via, pub rtt: Duration }
pub enum Via { Direct(SocketAddr), Relay(WldId) }
pub enum AttemptOutcome<T> { Connected { transport: T, via: Via, rtt: Duration }, Failed { via: Via } }
pub enum RaceError { NoCandidates, AllFailed, Deadline }
```

---

## 4. `race` 状態機械

`rank` で並べ替えた候補を direct / relay に partition し、以下を回す:

**arm の初期化**
- relay を **t=0 で全て先行 arm**（eager、§5）。
- 先頭 direct を 1 個 arm。
- direct が皆無なら relay を hold しない（`gate_open = true`。勝機を与える相手が居ない）。

**イベントループ（`tokio::select!`）**

| # | イベント                    | 動作                                                                    |
| - | --------------------------- | ----------------------------------------------------------------------- |
| 1 | `stagger` tick              | 次の direct を並行 arm（前が未完でも構わず）。タイマを reset。            |
| 2 | `gate`（`relay_handicap`）  | relay adoption を解禁。握手済 relay を hold 中なら **即採用**（failover +0 RTT）。 |
| 3 | 握手が resolve（`Connected`） | **direct** → 即採用（hold 中 relay より優先）。**relay** → gate 開なら即採用、閉なら `relay_ready` に hold。 |
| 3'| 握手が resolve（`Failed`）   | **direct 失敗**なら stagger を待たず**次 direct を即 arm**（RFC 8305 の fast-fail chain）。relay 失敗は放置。 |
| 4 | `overall_deadline`          | in-flight のまま時間切れ → 握手済 relay を拾うか [`RaceError::Deadline`]。 |

**終了判定**: `outstanding == 0 && directs.is_empty()`（進行中ゼロ かつ arm 待ち direct なし）→ 握手済 relay を hold していれば即採用（handicap を待たない）、無ければ [`RaceError::AllFailed`]。黒穴/拒否の timeout を待たずに返る。

### タイムライン例（`RaceCfg::default()`: stagger=250ms / handicap=500ms）

| シナリオ                                  | 結果                     | 所要（仮想時間） |
| ----------------------------------------- | ------------------------ | ---------------- |
| 先頭 direct が 50ms で成功                | direct 採用（d(2) は arm 前） | ~50ms            |
| 先頭 direct 黒穴・次 direct 80ms で成功    | 次 direct 採用           | ~330ms（stagger 250 + 80、黒穴 timeout 待たず） |
| 先頭 direct 10ms fast-fail・次 40ms 成功  | 次 direct 採用           | ~50ms（fail chain、stagger 待たず） |
| direct 両方黒穴・relay 30ms 握手済        | relay 採用（gate で）    | ~500ms（gate）、rtt=30ms |
| direct 両方 fast-fail・relay 30ms 握手済  | relay 早期採用           | ~30ms（全 direct 決着済）|
| relay 20ms 握手済・direct 100ms 成功      | **direct 採用**（hold 上書き） | ~100ms           |

---

## 5. eager relay — doctrine default（ADR-020 §S6、user 確定 2026-07-07）

relay 候補があれば **t=0 で先行 arm** する（既存 hub connection 上の open ≈ 1 RTT で握手が済む）。握手が済んでも `relay_handicap` 経過まで **採用を hold** し、in-flight の direct に勝機を与える:

- direct が hold 中に完了すれば **direct 採用**（relay は drop で tear down）。
- direct が全滅 or handicap 経過なら **relay を即採用** — relay は既に握手済なので failover は **+0 RTT**。

`relay_handicap = 0` なら relay を待たせず即採用、大きいほど direct 優先が強い。「relay を後から慌てて握る」のでなく「先に握って採用を遅らせる」ことで、direct 失敗時の failover コストをゼロにするのが狙い。

---

## 6. 勝者以外の tear down 契約（境界①）

engine は敗者を **future の drop** で cancel する（`FuturesUnordered` から落とすだけ）。ゆえに `Winner<T>` / 進行中 attempt が保持する transport `T` は、**Drop 時に relay stream を確実に close** する実装でなければならない:

- hub は dialer の `open` で `wld_id → ctx` の transient forwarding state を持つ（`unison_server.rs`）。放置すると握手ドロップ検知まで残り、D1 の「durable 0」を一瞬でも欠く。
- direct（`quinn::Connection`）は drop で UDP が閉じるため追加処理不要。

```rust
enum Transport { Direct(quinn::Connection), Relay(UnisonChannel) }
impl Drop for Transport { /* Relay は close して hub transient を即消す */ }
```

→ relay adapter を配線する次段（§8）で `Transport` enum と `Drop` を実装する。**この契約を破ると hub 側にゴミ state が溜まる**ため、relay 配線時の最重要チェック項目。

---

## 7. テスト戦略

| level        | 場所                                          | 何を担保するか                                       | 実行 |
| ------------ | --------------------------------------------- | ---------------------------------------------------- | ---- |
| **unit**     | `network::dial::tests`（10 件）               | engine の状態機械。**仮想時間**（`start_paused = true`）で scripted latency を `sleep` が演じ、実時間 0・決定論的に回る。 | 常時 |
| **medium**   | `tests/test_medium_connect_race.rs`（2 件）   | **実 quinn** で staggered race が死んだ decoy を飛ばし生きた world へ QUIC round-trip する。 | `#[ignore]`（`cargo test -- --ignored`） |

unit test が押さえる不変条件: first-direct-wins / 黒穴 direct = stagger 1 tick / fast-fail chain / eager relay の gate 採用 / 早期採用 / direct が hold relay に勝つ / AllFailed / Deadline / rank の relay-last。

---

## 8. Open points / 次段

- **relay adapter 配線** — `Transport` enum + `Drop`（§6）を実装し、`Candidate::Relay` を hub channel open に閉じ込める。これで direct 全滅時の relay failover が実効化する。
- **`rank` の高度化**（`rank` 内 TODO）:
  - 同一 prefix（/64, /48）共有の direct GUA を先頭へ（同一サイト・低 RTT 期待）。
  - IPv4 導入時は family interleave（v6, v4, …）で片 family 黒穴の頭独占を防ぐ。
- **win-cache**（ADR-020 Open point #1「庭師の actual reachability」）— `Winner.rtt` を実測 RTT として蓄積し、stagger を適応化（RFC 下限 100–150ms まで詰める）。
- **IPv4 / tailnet direct**（§D3）— 現状 IPv6 GUA のみ。adapter の family skip を解除する。

---

## 関連

- `crates/unison-protocol/src/network/dial.rs` — engine（`rank` / `race`）+ unit test
- `crates/unison-protocol/src/network/quic.rs` — `QuicClient::connect_race`（direct adapter）
- `crates/unison-protocol/src/network/client.rs` — `ProtocolClient::connect_race`（`after_connect` 共有）
- `crates/unison-protocol/tests/test_medium_connect_race.rs` — medium integration
- [server-initiated-stream.md](server-initiated-stream.md) — 同じ ADR-020 系（§S4 relay substrate）の SSOT
