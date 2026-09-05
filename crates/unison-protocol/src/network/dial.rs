//! `dial` — Happy Eyeballs v2 (RFC 8305) 型の staggered-race dialer。
//!
//! chronista-hub ADR-020 §S6 の consume 側。到達性 ladder（D3 `direct → relay`）を
//! *逐次フォールバック* でなく **1 本の時間差レース** に畳む。direct（別アドレス）と
//! relay（hub 経由の別到達機構）を「優先度と stagger だけ違う候補」に均し、最初に握手
//! 完了した経路を採用、残りは cancel する。「全滅」を誰も判定しない —— 遅延ノブは有界な
//! stagger だけで、per-endpoint timeout の「短すぎ＝良経路誤棄／長すぎ＝数秒待ち」
//! ジレンマが構造的に消える。
//!
//! # 設計: engine と I/O の分離（§S6 の data/calc/action）
//!
//! [`race`] は **network 非依存の generic engine**。実際の握手（quinn connect /
//! relay channel open）は呼び出し側が渡す `attempt` closure に閉じる。これにより
//! 並行タイミングの状態機械を仮想時間で決定論的にテストでき（本ファイル末尾）、
//! 実 I/O は薄い adapter に寄る。[`rank`] は副作用ゼロの候補ランキング。
//!
//! # eager relay（doctrine default、ADR-020 §S6 user 確定 2026-07-07）
//!
//! relay 候補があれば **t=0 で先行 arm**（既存 hub connection 上の open ≈1 RTT）し、
//! 握手済でも `relay_handicap` 経過まで **採用を hold** する。direct が hold 中に完了
//! すれば direct 採用（relay は drop で tear down）。direct が全滅 or handicap 経過なら
//! relay を即採用 —— relay は既に握手済なので failover は **+0 RTT**。
//!
//! # 勝者以外の tear down 契約（境界①）
//!
//! engine は敗者を **future の drop** で cancel する。`Winner<T>` / 進行中 attempt が
//! 保持する transport `T` は、**Drop 時に relay stream を確実に close** する実装で
//! なければならない（hub は dialer の `open` で `wld_id→ctx` の transient forwarding
//! state を持つ ── `unison_server.rs`。放置すると握手ドロップ検知まで残り、D1 の
//! 「durable 0」を一瞬でも欠く）。direct（quinn::Connection）は drop で UDP が閉じる。
//!
//! # 実 adapter の配線（次段: `ProtocolClient::connect_race` として）
//!
//! ```ignore
//! enum Transport { Direct(quinn::Connection), Relay(UnisonChannel) }
//! impl Drop for Transport { /* Relay は close して hub transient を即消す */ }
//!
//! // direct: 共有 client Endpoint から 1 発 connect（quic.rs:390 の core を切り出し）。
//! async fn direct_attempt(ep: &quinn::Endpoint, addr: SocketAddr, sni: &str)
//!     -> AttemptOutcome<Transport>
//! {
//!     let t0 = Instant::now();
//!     match ep.connect(addr, sni) {
//!         Ok(conn) => match conn.await {
//!             Ok(c)  => AttemptOutcome::Connected {
//!                 transport: Transport::Direct(c), via: Via::Direct(addr), rtt: t0.elapsed() },
//!             Err(_) => AttemptOutcome::Failed { via: Via::Direct(addr) },
//!         },
//!         Err(_) => AttemptOutcome::Failed { via: Via::Direct(addr) },
//!     }
//! }
//!
//! // relay: Discover で確立済の hub ProtocolClient を再利用。arm = open + declare（≈1 RTT）。
//! async fn relay_attempt(hub: &ProtocolClient, to: WldId) -> AttemptOutcome<Transport> {
//!     let t0 = Instant::now();
//!     match hub.open_channel("relay").await {                       // client.rs:175
//!         Ok(ch) => match ch.request::<_, serde_json::Value>("open", &serde_json::json!({"to": to})).await {
//!             Ok(_)  => AttemptOutcome::Connected {
//!                 transport: Transport::Relay(ch), via: Via::Relay(to), rtt: t0.elapsed() },
//!             Err(_) => AttemptOutcome::Failed { via: Via::Relay(to) },
//!         },
//!         Err(_) => AttemptOutcome::Failed { via: Via::Relay(to) },
//!     }
//! }
//! ```

use std::collections::VecDeque;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use tokio::time::{Instant, sleep};

/// location 独立の home-World 番地（ADR-020 D2）。relay 中継の宛先 routing key。
pub type WldId = String;

/// 到達候補。[`rank`] が優先度順に並べ、[`race`] が stagger を付けて arm する。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Candidate {
    /// direct: IPv6 GUA /（将来）IPv4 / tailnet。到達手段は「その addr へ QUIC 握手」だけ。
    /// 全滅しても hub は無関係（D3-a、hub data 不在）。
    Direct(SocketAddr),
    /// relay = universal floor（D3-b）。hub 経由の *別到達機構*。arm = hub conn 上に
    /// `relay` channel を開き `open{to}` 宣言。負け時は hub の transient state を tear down。
    Relay(WldId),
}

impl Candidate {
    /// direct 候補か（partition / arm スケジュールの分岐）。
    pub fn is_direct(&self) -> bool {
        matches!(self, Candidate::Direct(_))
    }

    /// この候補が勝った場合の観測種別（transport を剥がした [`Via`]）。adapter が
    /// `AttemptOutcome` を組むときの mapping にも使う。
    pub fn via(&self) -> Via {
        match self {
            Candidate::Direct(a) => Via::Direct(*a),
            Candidate::Relay(w) => Via::Relay(w.clone()),
        }
    }
}

/// レースのチューニング。doctrine default = **eager relay arm**（ADR-020 §S6）。
#[derive(Clone, Debug)]
pub struct RaceCfg {
    /// direct 候補間の stagger（RFC 8305 Connection Attempt Delay）。QUIC 1-RTT 基準で
    /// 250ms は緩め。RFC 下限 100–150ms まで詰め可（win-cache の実測 RTT で適応化）。
    pub stagger: Duration,
    /// eager relay の adoption gate。握手済 relay をここまで hold し、in-flight direct に
    /// 勝機を与える。0 なら relay を待たせず即採用、大きいほど direct 優先が強い。
    pub relay_handicap: Duration,
    /// 全体の hard deadline。越えたら握手済 relay を拾うか [`RaceError::Deadline`]。
    pub overall_deadline: Duration,
}

impl Default for RaceCfg {
    fn default() -> Self {
        Self {
            stagger: Duration::from_millis(250),
            relay_handicap: Duration::from_millis(500),
            overall_deadline: Duration::from_secs(10),
        }
    }
}

/// 採用された経路。`transport` は勝った I/O ハンドル、`via`/`rtt` は win-cache・庭師供給用。
#[derive(Debug)]
pub struct Winner<T> {
    pub transport: T,
    pub via: Via,
    /// 勝者候補の握手 RTT（win-cache → ADR-020 Open point #1 庭師の "actual reachability"）。
    pub rtt: Duration,
}

/// 勝った経路の種別（`Candidate` から transport を剥がした観測値）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    Direct(SocketAddr),
    Relay(WldId),
}

/// 1 候補の握手結果。`attempt` closure が返す。
#[derive(Debug)]
pub enum AttemptOutcome<T> {
    Connected {
        transport: T,
        via: Via,
        rtt: Duration,
    },
    Failed {
        via: Via,
    },
}

/// レースの失敗。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RaceError {
    #[error("候補が空")]
    NoCandidates,
    /// 全候補が握手失敗（黒穴/拒否）。deadline を待たず即返す。
    #[error("全候補が握手失敗")]
    AllFailed,
    /// deadline までにどの候補も握手完了しなかった（in-flight のまま時間切れ）。
    #[error("deadline 超過")]
    Deadline,
}

/// hub の opaque 順序付き候補（D2）に *消費側で* 意味を与える純関数（副作用ゼロ）。
///
/// 規則:
/// - 同一 prefix（/64,/48）共有の direct GUA = 同一サイト → 先頭（低 RTT 期待）〔TODO〕
/// - IPv4 導入時は family interleave（v6,v4,…）で片 family 黒穴の頭独占を防ぐ〔TODO〕
/// - relay は最劣後（末尾）
///
/// 現状は IPv6 GUA のみ運用（ADR-020 §D3、IPv4 deferred）ゆえ direct は順序保持で足りる。
fn rank(candidates: Vec<Candidate>, my_addrs: &[IpAddr]) -> Vec<Candidate> {
    let (direct, relay): (Vec<_>, Vec<_>) = candidates.into_iter().partition(Candidate::is_direct);
    // TODO(§S6): direct.sort_by_key(|c| prefix_distance(c, my_addrs))  // 同一サイト優先
    // TODO(§S6): family interleave（IPv4 導入後）
    let _ = my_addrs;
    direct.into_iter().chain(relay).collect()
}

/// 順序付き候補を stagger 付きで並行 arm し、最初に握手完了した経路を採用する（HEv2）。
///
/// `attempt` は 1 候補の握手を行う closure。全呼び出しが同一の future 型を返すため
/// boxing 不要（`FuturesUnordered<Fut>`）。engine は `Candidate` の direct/relay 区別と
/// タイマだけを見て、I/O は一切知らない。
pub async fn race<T, F, Fut>(
    candidates: Vec<Candidate>,
    my_addrs: &[IpAddr],
    cfg: RaceCfg,
    mut attempt: F,
) -> Result<Winner<T>, RaceError>
where
    F: FnMut(Candidate) -> Fut,
    Fut: Future<Output = AttemptOutcome<T>>,
{
    let ranked = rank(candidates, my_addrs);
    if ranked.is_empty() {
        return Err(RaceError::NoCandidates);
    }

    let (direct_v, relays): (Vec<_>, Vec<_>) = ranked.into_iter().partition(Candidate::is_direct);
    let mut directs: VecDeque<Candidate> = direct_v.into();
    let had_direct = !directs.is_empty();

    let mut inflight = FuturesUnordered::new();
    let mut outstanding = 0usize;

    // eager: relay(s) を t=0 で先行 arm。
    for r in relays {
        inflight.push(attempt(r));
        outstanding += 1;
    }
    // 最初の direct を arm。
    if let Some(d) = directs.pop_front() {
        inflight.push(attempt(d));
        outstanding += 1;
    }

    // direct 候補が皆無なら relay を hold しない（勝機を与える相手が居ない）。
    let mut gate_open = !had_direct;
    let mut relay_ready: Option<Winner<T>> = None;

    let stagger = sleep(cfg.stagger);
    tokio::pin!(stagger);
    let gate = sleep(cfg.relay_handicap);
    tokio::pin!(gate);
    let deadline = sleep(cfg.overall_deadline);
    tokio::pin!(deadline);

    loop {
        // 終了判定: 進行中ゼロ かつ arm 待ち direct なし。
        //   → 握手済 relay を hold していれば即採用（handicap を待たない）、無ければ全滅。
        if outstanding == 0 && directs.is_empty() {
            return relay_ready.ok_or(RaceError::AllFailed);
        }

        tokio::select! {
            // (1) stagger tick → 次の direct を並行 arm（前が未完でも構わず）。
            _ = &mut stagger => {
                if let Some(d) = directs.pop_front() {
                    inflight.push(attempt(d));
                    outstanding += 1;
                }
                stagger.as_mut().reset(Instant::now() + cfg.stagger);
            }

            // (2) relay adoption gate。握手済 relay があれば即採用（failover +0 RTT）。
            _ = &mut gate, if !gate_open => {
                gate_open = true;
                if let Some(w) = relay_ready.take() {
                    return Ok(w); // 敗者は inflight drop で cancel（tear down は T::drop）
                }
            }

            // (3) いずれかの握手が resolve。
            Some(outcome) = inflight.next() => {
                outstanding -= 1;
                match outcome {
                    AttemptOutcome::Connected { transport, via, rtt } => {
                        let winner = Winner { transport, via: via.clone(), rtt };
                        match via {
                            // direct は即採用（held relay があっても direct 優先）。
                            Via::Direct(_) => return Ok(winner),
                            Via::Relay(_) => {
                                if gate_open {
                                    return Ok(winner);
                                }
                                relay_ready = Some(winner); // eager: 握手済を hold
                            }
                        }
                    }
                    // 黒穴/拒否は待たない。direct 失敗なら次 direct を *即* arm（RFC 8305）。
                    AttemptOutcome::Failed { via } => {
                        if matches!(via, Via::Direct(_))
                            && let Some(d) = directs.pop_front()
                        {
                            inflight.push(attempt(d));
                            outstanding += 1;
                        }
                    }
                }
            }

            // (4) hard deadline。in-flight のまま時間切れ → 握手済 relay を拾うか Deadline。
            _ = &mut deadline => {
                return relay_ready.take().ok_or(RaceError::Deadline);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 決定論テスト（仮想時間 = `#[tokio::test(start_paused = true)]`）。
    //! fake `attempt` が scripted な握手 latency を `sleep` で演じる。tokio が全タスク
    //! idle 時に仮想時計を次のタイマまで自動前進させるため、実時間 0 で確定的に回る。

    use super::*;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv6Addr, SocketAddr};
    use std::time::Duration;
    use tokio::time::{Instant, sleep};

    fn d(port: u16) -> Candidate {
        Candidate::Direct(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port))
    }
    fn r(id: &str) -> Candidate {
        Candidate::Relay(id.to_string())
    }

    #[derive(Clone)]
    enum Script {
        /// `delay` 後に握手成功。
        Ok(Duration),
        /// `delay` 後に握手失敗（黒穴は大きい delay で表す）。
        Fail(Duration),
    }

    /// script から uniform な future 型を返す attempt closure を作る。`T = Candidate`
    /// （どの候補が勝ったかを transport で assert する）。
    fn attempter(
        script: HashMap<Candidate, Script>,
    ) -> impl FnMut(Candidate) -> std::pin::Pin<Box<dyn Future<Output = AttemptOutcome<Candidate>>>>
    {
        move |c: Candidate| {
            let s = script.get(&c).cloned();
            Box::pin(async move {
                let via = c.via();
                match s {
                    Some(Script::Ok(delay)) => {
                        sleep(delay).await;
                        AttemptOutcome::Connected {
                            transport: c,
                            via,
                            rtt: delay,
                        }
                    }
                    Some(Script::Fail(delay)) => {
                        sleep(delay).await;
                        AttemptOutcome::Failed { via }
                    }
                    None => AttemptOutcome::Failed { via }, // 未 script = 即失敗
                }
            })
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// 経過（仮想）時間が `expect` ± 40ms に入るか。
    fn near(elapsed: Duration, expect: u64) -> bool {
        let e = elapsed.as_millis() as i64;
        (e - expect as i64).abs() <= 40
    }

    // ── pure ────────────────────────────────────────────────────────────────

    #[test]
    fn rank_puts_relay_last_and_keeps_direct_order() {
        let out = rank(vec![r("h"), d(1), d(2)], &[]);
        assert_eq!(out, vec![d(1), d(2), r("h")]);
    }

    #[tokio::test(start_paused = true)]
    async fn empty_is_no_candidates() {
        let res = race(vec![], &[], RaceCfg::default(), attempter(HashMap::new())).await;
        assert_eq!(res.unwrap_err(), RaceError::NoCandidates);
    }

    // ── happy path ────────────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn first_direct_wins_without_racing() {
        let script = HashMap::from([(d(1), Script::Ok(ms(50)))]);
        let t0 = Instant::now();
        let w = race(vec![d(1), d(2)], &[], RaceCfg::default(), attempter(script))
            .await
            .unwrap();
        assert_eq!(
            w.via,
            Via::Direct(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 1))
        );
        // 50ms < stagger 250ms → d(2) は arm される前に決着。
        assert!(near(t0.elapsed(), 50), "elapsed = {:?}", t0.elapsed());
    }

    // ── dead direct のコスト = stagger 1 tick ────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn blackhole_direct_costs_one_stagger_tick() {
        // d(1) は黒穴（deadline 手前まで無応答）、d(2) が生きている。
        let script = HashMap::from([(d(1), Script::Fail(ms(9_000))), (d(2), Script::Ok(ms(80)))]);
        let t0 = Instant::now();
        let w = race(vec![d(1), d(2)], &[], RaceCfg::default(), attempter(script))
            .await
            .unwrap();
        assert_eq!(
            w.via,
            Via::Direct(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 2))
        );
        // d(2) は stagger 250ms で arm → 80ms 後 = 330ms に完了。黒穴 timeout を待たない。
        assert!(near(t0.elapsed(), 330), "elapsed = {:?}", t0.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn fast_fail_direct_chains_next_immediately() {
        // d(1) が即失敗 → stagger を待たず d(2) を即 arm。
        let script = HashMap::from([(d(1), Script::Fail(ms(10))), (d(2), Script::Ok(ms(40)))]);
        let t0 = Instant::now();
        let w = race(vec![d(1), d(2)], &[], RaceCfg::default(), attempter(script))
            .await
            .unwrap();
        assert_eq!(
            w.via,
            Via::Direct(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 2))
        );
        assert!(
            near(t0.elapsed(), 50),
            "elapsed = {:?} (10 fail + 40 ok)",
            t0.elapsed()
        );
    }

    // ── eager relay ──────────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn eager_relay_adopts_at_gate_when_directs_blackhole() {
        // direct 両方黒穴、relay は 30ms で握手済 → gate 500ms で採用（+0 RTT）。
        let script = HashMap::from([
            (d(1), Script::Fail(ms(9_000))),
            (d(2), Script::Fail(ms(9_000))),
            (r("hub"), Script::Ok(ms(30))),
        ]);
        let t0 = Instant::now();
        let w = race(
            vec![d(1), d(2), r("hub")],
            &[],
            RaceCfg::default(),
            attempter(script),
        )
        .await
        .unwrap();
        assert_eq!(w.via, Via::Relay("hub".into()));
        assert_eq!(w.rtt, ms(30)); // relay 握手は 30ms（gate は adoption を遅らせるだけ）
        assert!(
            near(t0.elapsed(), 500),
            "elapsed = {:?} (gate)",
            t0.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn eager_relay_adopts_early_when_directs_fail_fast() {
        // direct が確定的に即失敗 → handicap 全部を待たず relay を早期採用。
        let script = HashMap::from([
            (d(1), Script::Fail(ms(10))),
            (d(2), Script::Fail(ms(10))),
            (r("hub"), Script::Ok(ms(30))),
        ]);
        let t0 = Instant::now();
        let w = race(
            vec![d(1), d(2), r("hub")],
            &[],
            RaceCfg::default(),
            attempter(script),
        )
        .await
        .unwrap();
        assert_eq!(w.via, Via::Relay("hub".into()));
        // d(1)@10 fail→d(2) arm@10→fail@20、relay@30 完了で全 direct 決着済 → 30ms 採用。
        assert!(
            near(t0.elapsed(), 30),
            "elapsed = {:?} (early)",
            t0.elapsed()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn direct_beats_already_connected_relay() {
        // relay は 20ms で握手済（hold）、direct は 100ms で完了 → direct 優先。
        let script = HashMap::from([(r("hub"), Script::Ok(ms(20))), (d(1), Script::Ok(ms(100)))]);
        let t0 = Instant::now();
        let w = race(
            vec![d(1), r("hub")],
            &[],
            RaceCfg::default(),
            attempter(script),
        )
        .await
        .unwrap();
        assert_eq!(
            w.via,
            Via::Direct(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 1))
        );
        assert!(near(t0.elapsed(), 100), "elapsed = {:?}", t0.elapsed());
    }

    // ── 失敗系 ────────────────────────────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn all_direct_fail_no_relay_is_all_failed() {
        let script = HashMap::from([(d(1), Script::Fail(ms(10)))]);
        let res = race(vec![d(1)], &[], RaceCfg::default(), attempter(script)).await;
        assert_eq!(res.unwrap_err(), RaceError::AllFailed);
    }

    #[tokio::test(start_paused = true)]
    async fn blackhole_without_relay_hits_deadline() {
        let script = HashMap::from([(d(1), Script::Fail(ms(9_000)))]); // deadline より長い
        let cfg = RaceCfg {
            overall_deadline: ms(1_000),
            ..Default::default()
        };
        let t0 = Instant::now();
        let res = race(vec![d(1)], &[], cfg, attempter(script)).await;
        assert_eq!(res.unwrap_err(), RaceError::Deadline);
        assert!(near(t0.elapsed(), 1_000), "elapsed = {:?}", t0.elapsed());
    }
}
