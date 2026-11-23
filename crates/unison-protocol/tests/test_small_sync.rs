//! Small テスト: `unison::sync` の同期コア（`Slots` / `Origin` / `Subscription` / `Slave` / `Relay`）と
//! KDL スキーマ `SYNC_KDL`（IO なし）。
//!
//! Memory: mem_1ChKdJTLL17QHAFpktCt6S

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Semaphore;

use unison::parser::SchemaParser;
use unison::sync::{
    CONTROL_CHANNEL, FEED_CHANNEL, Origin, Relay, SYNC_KDL, SetFuture, Slave, SlotState,
    SlotUpdate, Slots, Snapshot, SyncError, Upstream,
};

const PATIENCE: Duration = Duration::from_secs(5);

fn update(life: u64, slot: &str, rev: u64, value: Value) -> SlotUpdate {
    SlotUpdate {
        life,
        slot: slot.into(),
        rev,
        value,
    }
}

fn state(rev: u64, value: Value) -> SlotState {
    SlotState { rev, value }
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    tokio::time::timeout(PATIENCE, async {
        while !done() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("時間切れ: {what}"));
}

async fn within<T>(what: &str, work: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, work)
        .await
        .unwrap_or_else(|_| panic!("時間切れ: {what}"))
}

// ── Schema ──

#[test]
fn sync_kdl_parses_cleanly() {
    let parsed = SchemaParser::new()
        .parse(SYNC_KDL)
        .expect("SYNC_KDL must parse without errors");
    let protocol = parsed.protocol.expect("protocol node");
    assert_eq!(protocol.name, "unison-sync");
    let names: Vec<&str> = protocol.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec![CONTROL_CHANNEL, FEED_CHANNEL]);
}

// ── Slots ──

#[test]
fn set_advances_rev_per_slot() {
    let mut slots = Slots::new(1);
    assert_eq!(
        slots.set("light", json!(1)),
        update(1, "light", 1, json!(1))
    );
    assert_eq!(
        slots.set("light", json!(2)),
        update(1, "light", 2, json!(2))
    );
    assert_eq!(
        slots.set("camera", json!("wide")),
        update(1, "camera", 1, json!("wide"))
    );
    assert_eq!(slots.get("light"), Some(&state(2, json!(2))));
    assert_eq!(slots.get("camera"), Some(&state(1, json!("wide"))));
    assert_eq!(slots.get("sound"), None);
}

#[test]
fn out_of_order_is_ignored() {
    let mut slots = Slots::new(1);
    assert!(slots.merge(&update(1, "light", 2, json!("b"))));
    assert!(!slots.merge(&update(1, "light", 1, json!("a"))));
    assert!(!slots.merge(&update(1, "light", 2, json!("b"))));
    assert_eq!(slots.get("light"), Some(&state(2, json!("b"))));
}

#[test]
fn stale_life_is_ignored() {
    let mut slots = Slots::new(2);
    assert!(!slots.merge(&update(1, "light", 5, json!("old"))));
    assert_eq!(slots.get("light"), None);
    assert!(slots.merge(&update(2, "light", 1, json!("new"))));
}

#[test]
fn rev_zero_is_ignored() {
    let mut slots = Slots::new(1);
    assert!(!slots.merge(&update(1, "light", 0, json!(1))));
    assert_eq!(slots.get("light"), None);
}

#[test]
fn slots_merge_in_any_order() {
    let mut origin = Slots::new(1);
    let light = origin.set("light", json!(1));
    let camera = origin.set("camera", json!(2));
    let mut slave = Slots::new(1);
    assert!(slave.merge(&camera));
    assert!(slave.merge(&light));
    assert_eq!(slave.snapshot(), origin.snapshot());
}

#[test]
fn changes_since_folds_to_latest_in_change_order() {
    let mut slots = Slots::new(1);
    slots.set("light", json!(1));
    let since = slots.seq();
    slots.set("camera", json!(1));
    slots.set("light", json!(2));
    slots.set("light", json!(3));
    assert_eq!(
        slots.changes_since(since),
        [
            update(1, "camera", 1, json!(1)),
            update(1, "light", 3, json!(3)),
        ]
    );
    assert_eq!(slots.changes_since(slots.seq()), []);
}

#[test]
fn snapshot_round_trips() {
    let mut slots = Slots::new(7);
    slots.set("light", json!(1));
    slots.set("camera", json!({ "x": 1 }));
    let again = Slots::from_snapshot(slots.snapshot());
    assert_eq!(again.snapshot(), slots.snapshot());
    assert_eq!(again.life(), 7);
}

// ── Origin と購読 ──

#[tokio::test]
async fn subscribe_gets_snapshot_then_updates() {
    let origin = Origin::new(1);
    origin.set("light", json!(1)).unwrap();
    let (snapshot, mut subscription) = origin.subscribe();
    assert_eq!(snapshot.slots.get("light"), Some(&state(1, json!(1))));
    origin.set("light", json!(2)).unwrap();
    assert_eq!(
        within("変動", subscription.next()).await,
        Some(vec![update(1, "light", 2, json!(2))])
    );
}

#[tokio::test]
async fn slow_subscriber_gets_latest_per_slot() {
    let origin = Origin::new(1);
    let (_, mut subscription) = origin.subscribe();
    for i in 1..=5 {
        origin.set("light", json!(i)).unwrap();
    }
    origin.set("camera", json!("wide")).unwrap();
    assert_eq!(
        within("変動", subscription.next()).await,
        Some(vec![
            update(1, "light", 5, json!(5)),
            update(1, "camera", 1, json!("wide")),
        ])
    );
}

#[tokio::test]
async fn close_ends_subscription_after_pending_changes() {
    let origin = Origin::new(1);
    let (_, mut subscription) = origin.subscribe();
    origin.set("light", json!(1)).unwrap();
    origin.close();
    assert!(origin.is_closed());
    assert_eq!(
        within("残りの変動", subscription.next()).await,
        Some(vec![update(1, "light", 1, json!(1))])
    );
    assert_eq!(within("終わり", subscription.next()).await, None);
}

#[test]
fn cannot_set_closed() {
    let origin = Origin::new(1);
    origin.close();
    assert_eq!(origin.set("light", json!(1)), Err(SyncError::Closed));
}

#[tokio::test]
async fn empty_slot_name_is_refused_everywhere() {
    let origin = Origin::new(1);
    assert_eq!(
        origin.set("", json!(1)),
        Err(SyncError::Refused("bad_request".into()))
    );
    let slave = Slave::follow(&origin);
    assert_eq!(
        slave.set("", json!(1)).await,
        Err(SyncError::Refused("bad_request".into()))
    );
    let relay = Relay::from_slave(slave);
    assert_eq!(
        relay.set("", json!(1)).await,
        Err(SyncError::Refused("bad_request".into()))
    );
    assert!(origin.snapshot().slots.is_empty());
}

// ── Slave ──

#[tokio::test]
async fn join_gets_snapshot_then_updates() {
    let origin = Origin::new(3);
    origin.set("light", json!(1)).unwrap();
    let mut slave = Slave::follow(&origin);
    assert_eq!(slave.life(), 3);
    assert_eq!(slave.get("light"), Some(state(1, json!(1))));
    origin.set("camera", json!("wide")).unwrap();
    assert_eq!(
        within("変動", slave.changed()).await,
        Some(vec![update(3, "camera", 1, json!("wide"))])
    );
    assert_eq!(slave.snapshot(), origin.snapshot());
}

#[tokio::test]
async fn slave_set_is_confirmed_by_origin() {
    let origin = Origin::new(1);
    let writer = Slave::follow(&origin);
    let mut other = Slave::follow(&origin);
    let confirmed = within("頼む", writer.set("light", json!(2))).await.unwrap();
    assert_eq!(confirmed, update(1, "light", 1, json!(2)));
    assert_eq!(writer.get("light"), Some(state(1, json!(2))));
    assert_eq!(origin.get("light"), Some(state(1, json!(2))));
    assert_eq!(within("変動", other.changed()).await, Some(vec![confirmed]));
}

#[tokio::test]
async fn origin_decides_concurrent_sets() {
    let origin = Origin::new(1);
    let a = Slave::follow(&origin);
    let b = Slave::follow(&origin);
    let (first, second) = tokio::join!(a.set("light", json!("a")), b.set("light", json!("b")));
    let (first, second) = (first.unwrap(), second.unwrap());
    let winner = if first.rev > second.rev {
        first
    } else {
        second
    };
    assert_eq!(winner.rev, 2);
    assert_eq!(origin.get("light"), Some(state(2, winner.value.clone())));
    until("全員が揃う", || {
        a.get("light") == origin.get("light") && b.get("light") == origin.get("light")
    })
    .await;
}

struct Canned(SlotUpdate);

impl Upstream for Canned {
    fn set(&self, _slot: String, _value: Value) -> SetFuture<'_> {
        Box::pin(std::future::ready(Ok(self.0.clone())))
    }
}

#[tokio::test]
async fn late_reply_is_ignored() {
    let (slave, inlet) = Slave::new(
        Snapshot {
            life: 1,
            slots: BTreeMap::new(),
        },
        Arc::new(Canned(update(1, "light", 1, json!("a")))),
    );
    assert!(inlet.apply(&update(1, "light", 2, json!("b"))));
    let confirmed = slave.set("light", json!("a")).await.unwrap();
    assert_eq!(confirmed.rev, 1);
    assert_eq!(slave.get("light"), Some(state(2, json!("b"))));
}

#[tokio::test]
async fn reply_first_then_feed() {
    let (mut slave, inlet) = Slave::new(
        Snapshot {
            life: 1,
            slots: BTreeMap::new(),
        },
        Arc::new(Canned(update(1, "light", 1, json!("a")))),
    );
    slave.set("light", json!("a")).await.unwrap();
    assert_eq!(slave.get("light"), Some(state(1, json!("a"))));
    assert!(!inlet.apply(&update(1, "light", 1, json!("a"))));
    assert_eq!(
        within("変動", slave.changed()).await,
        Some(vec![update(1, "light", 1, json!("a"))])
    );
    inlet.close();
    assert_eq!(within("終わり", slave.changed()).await, None);
}

struct Silent;

impl Upstream for Silent {
    fn set(&self, _slot: String, _value: Value) -> SetFuture<'_> {
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn pending_set_ends_when_slave_is_closed() {
    let (slave, inlet) = Slave::new(Snapshot::default(), Arc::new(Silent));
    let slave = Arc::new(slave);
    let pending = tokio::spawn({
        let slave = slave.clone();
        async move { slave.set("light", json!(1)).await }
    });
    tokio::task::yield_now().await;
    inlet.close();
    assert_eq!(
        within("頼みが終わる", pending).await.unwrap(),
        Err(SyncError::Closed)
    );
}

struct Counting(Arc<AtomicUsize>);

impl Upstream for Counting {
    fn set(&self, _slot: String, _value: Value) -> SetFuture<'_> {
        let polls = self.0.clone();
        Box::pin(std::future::poll_fn(move |_| {
            polls.fetch_add(1, Ordering::Relaxed);
            std::task::Poll::Pending
        }))
    }
}

#[tokio::test]
async fn pending_set_is_not_woken_by_updates() {
    let polls = Arc::new(AtomicUsize::new(0));
    let snapshot = Snapshot {
        life: 1,
        ..Snapshot::default()
    };
    let (slave, inlet) = Slave::new(snapshot, Arc::new(Counting(polls.clone())));
    let slave = Arc::new(slave);
    let pending = tokio::spawn({
        let slave = slave.clone();
        async move { slave.set("light", json!(0)).await }
    });
    until("頼みが上流へ出る", || {
        polls.load(Ordering::Relaxed) > 0
    })
    .await;
    let before = polls.load(Ordering::Relaxed);
    for rev in 1..=100 {
        assert!(inlet.apply(&update(1, "camera", rev, json!(rev))));
        tokio::task::yield_now().await;
    }
    assert_eq!(polls.load(Ordering::Relaxed), before);
    inlet.close();
    assert_eq!(
        within("頼みが終わる", pending).await.unwrap(),
        Err(SyncError::Closed)
    );
}

#[tokio::test]
async fn pending_relayed_set_ends_when_relay_leaves() {
    let (relay, inlet) = Slave::new(Snapshot::default(), Arc::new(Silent));
    tokio::spawn(async move { inlet.slave_gone().await });
    let downstream = Arc::new(Slave::follow(&relay));
    let pending = tokio::spawn({
        let downstream = downstream.clone();
        async move { downstream.set("light", json!(1)).await }
    });
    tokio::task::yield_now().await;
    drop(relay);
    assert_eq!(
        within("頼みが終わる", pending).await.unwrap(),
        Err(SyncError::Closed)
    );
}

#[tokio::test]
async fn stale_life_update_is_ignored_by_slave() {
    let (slave, inlet) = Slave::new(
        Snapshot {
            life: 2,
            slots: BTreeMap::new(),
        },
        Arc::new(Canned(update(2, "light", 1, json!(1)))),
    );
    assert!(!inlet.apply(&update(1, "light", 9, json!("old"))));
    assert_eq!(slave.get("light"), None);
}

#[tokio::test]
async fn slow_consumer_slave_is_always_current() {
    let origin = Origin::new(1);
    let mut slave = Slave::follow(&origin);
    for i in 1..=100 {
        origin.set("light", json!(i)).unwrap();
        tokio::task::yield_now().await;
    }
    until("Slave が追いつく", || {
        slave.get("light") == Some(state(100, json!(100)))
    })
    .await;
    assert_eq!(
        within("変動", slave.changed()).await,
        Some(vec![update(1, "light", 100, json!(100))])
    );
}

// ── インメモリ中継 (Slave::follow) ──

#[tokio::test]
async fn relay_delivers_downstream() {
    let origin = Origin::new(1);
    let relay = Slave::follow(&origin);
    let mut downstream = Slave::follow(&relay);
    origin.set("light", json!(1)).unwrap();
    assert_eq!(
        within("変動", downstream.changed()).await,
        Some(vec![update(1, "light", 1, json!(1))])
    );
}

#[tokio::test]
async fn relay_chain() {
    let origin = Origin::new(1);
    let first = Slave::follow(&origin);
    let second = Slave::follow(&first);
    let mut third = Slave::follow(&second);
    origin.set("camera", json!(2)).unwrap();
    assert_eq!(
        within("変動", third.changed()).await,
        Some(vec![update(1, "camera", 1, json!(2))])
    );
}

#[tokio::test]
async fn relay_join_gets_relay_state() {
    let origin = Origin::new(1);
    origin.set("light", json!(1)).unwrap();
    let relay = Slave::follow(&origin);
    let downstream = Slave::follow(&relay);
    assert_eq!(downstream.snapshot(), relay.snapshot());
}

#[tokio::test]
async fn relayed_slave_can_set() {
    let origin = Origin::new(1);
    let relay = Slave::follow(&origin);
    let downstream = Slave::follow(&relay);
    let confirmed = within("頼む", downstream.set("light", json!(2)))
        .await
        .unwrap();
    assert_eq!(confirmed, update(1, "light", 1, json!(2)));
    assert_eq!(origin.get("light"), Some(state(1, json!(2))));
    assert_eq!(relay.get("light"), Some(state(1, json!(2))));
    assert_eq!(downstream.get("light"), Some(state(1, json!(2))));
}

#[tokio::test]
async fn relay_leave_drops_downstream() {
    let origin = Origin::new(1);
    let relay = Slave::follow(&origin);
    let mut downstream = Slave::follow(&relay);
    drop(relay);
    assert_eq!(within("外れる", downstream.changed()).await, None);
    assert!(downstream.is_closed());
    assert_eq!(
        downstream.set("light", json!(1)).await,
        Err(SyncError::Closed)
    );
}

#[tokio::test]
async fn stop_drops_everyone() {
    let origin = Origin::new(1);
    let mut relay = Slave::follow(&origin);
    let mut downstream = Slave::follow(&relay);
    origin.close();
    assert_eq!(within("外れる", relay.changed()).await, None);
    assert_eq!(within("外れる", downstream.changed()).await, None);
}

// ── Relay ハンドル ──

struct Gate {
    open: Semaphore,
    rev: AtomicU64,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Semaphore::new(0),
            rev: AtomicU64::new(0),
        })
    }
}

impl Upstream for Gate {
    fn set(&self, slot: String, value: Value) -> SetFuture<'_> {
        Box::pin(async move {
            if slot == "slow" {
                let _ = self.open.acquire().await;
            }
            Ok(SlotUpdate {
                life: 1,
                slot,
                rev: self.rev.fetch_add(1, Ordering::Relaxed) + 1,
                value,
            })
        })
    }
}

fn relay_behind(gate: Arc<Gate>) -> Relay {
    let snapshot = Snapshot {
        life: 1,
        ..Snapshot::default()
    };
    let (slave, inlet) = Slave::new(snapshot, gate);
    let pump = tokio::spawn(async move { inlet.slave_gone().await });
    Relay::hold(slave, Some(pump))
}

#[tokio::test]
async fn relay_handle_join_gets_state_then_updates() {
    let origin = Origin::new(1);
    origin.set("light", json!(1)).unwrap();
    let relay = Relay::from_slave(Slave::follow(&origin));
    assert_eq!(relay.life(), 1);

    let (snapshot, mut presence) = relay.join().unwrap();
    assert_eq!(snapshot.life, 1);
    assert_eq!(snapshot.slots.get("light"), Some(&state(1, json!(1))));

    origin.set("camera", json!("wide")).unwrap();
    assert_eq!(
        within("変動", presence.next()).await,
        Some(vec![update(1, "camera", 1, json!("wide"))])
    );
}

#[tokio::test]
async fn relay_handle_set_is_confirmed_upstream_and_merged() {
    let origin = Origin::new(1);
    let relay = Relay::from_slave(Slave::follow(&origin));
    let (_, mut presence) = relay.join().unwrap();

    let confirmed = within("頼む", relay.set("light", json!(2))).await.unwrap();
    assert_eq!((confirmed.rev, &confirmed.value), (1, &json!(2)));
    assert_eq!(origin.get("light"), Some(state(1, json!(2))));
    assert_eq!(
        relay.snapshot().unwrap().slots.get("light"),
        Some(&state(1, json!(2)))
    );
    assert_eq!(within("変動", presence.next()).await, Some(vec![confirmed]));
}

#[tokio::test]
async fn relay_handle_set_waiting_upstream_blocks_nobody() {
    let gate = Gate::new();
    let relay = relay_behind(gate.clone());
    let slow = tokio::spawn({
        let relay = relay.clone();
        async move { relay.set("slow", json!(1)).await }
    });
    tokio::task::yield_now().await;

    let (_, _presence) = relay.join().unwrap();
    let fast = within("ほかの頼み", relay.set("fast", json!(2)))
        .await
        .unwrap();
    assert_eq!(fast.slot, "fast");
    assert!(!slow.is_finished());

    gate.open.add_permits(1);
    let slow = within("待っていた頼み", slow).await.unwrap().unwrap();
    assert_eq!(slow.slot, "slow");
    assert_eq!(relay.snapshot().unwrap().slots.len(), 2);
}

#[tokio::test]
async fn relay_handle_leaving_visitor_is_not_counted() {
    let origin = Origin::new(1);
    let relay = Relay::from_slave(Slave::follow(&origin));
    let (_, a) = relay.join().unwrap();
    let (_, _b) = relay.join().unwrap();
    assert_eq!(relay.visitors(), 2);
    drop(a);
    assert_eq!(relay.visitors(), 1);
    assert!(relay.set("light", json!(1)).await.is_ok());
}

#[tokio::test]
async fn relay_handle_upstream_stop_drops_visitors() {
    let origin = Origin::new(1);
    let relay = Relay::from_slave(Slave::follow(&origin));
    let (_, mut presence) = relay.join().unwrap();
    let mut watching = relay.observe().unwrap();

    origin.close();
    assert_eq!(within("外れる", presence.next()).await, None);
    assert_eq!(within("外れる", watching.next()).await, None);
    assert!(relay.is_stopped());
    assert!(matches!(relay.join(), Err(SyncError::Closed)));
    assert_eq!(relay.set("light", json!(1)).await, Err(SyncError::Closed));
}

#[tokio::test]
async fn relay_handle_stop_leaves_upstream_and_drops_visitors() {
    let origin = Origin::new(1);
    let relay = Relay::from_slave(Slave::follow(&origin));
    let (_, mut presence) = relay.join().unwrap();

    relay.stop().await;
    assert_eq!(within("外れる", presence.next()).await, None);
    assert!(relay.is_stopped());
    assert!(matches!(relay.join(), Err(SyncError::Closed)));
    assert_eq!(relay.set("light", json!(1)).await, Err(SyncError::Closed));
    assert!(relay.snapshot().is_none());
    assert!(relay.observe().is_none());
    relay.stop().await;
    assert!(origin.set("light", json!(1)).is_ok());
}

#[tokio::test]
async fn relay_handle_stop_ends_pending_set() {
    let relay = relay_behind(Gate::new());
    let slow = tokio::spawn({
        let relay = relay.clone();
        async move { relay.set("slow", json!(1)).await }
    });
    tokio::task::yield_now().await;
    within("止める", relay.stop()).await;
    assert_eq!(
        within("頼みが終わる", slow).await.unwrap(),
        Err(SyncError::Closed)
    );
}
