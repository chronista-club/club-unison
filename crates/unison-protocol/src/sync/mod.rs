//! Origin ↔ Slave の自動同期（同期コア・中継・QUIC 2 チャネル運び方）。
//!
//! 仕様: `spec/05-sync/SPEC.md` ／ 設計: `design/sync.md`
//!
//! - 状態は、名前の付いた要素（slot）の集まり。要素ごとに `(rev, value)` を持つ
//! - 正本は [`Origin`] の 1 つだけ。[`Origin::set`] が、その要素の rev を 1 つ進めて確定する
//! - 手元の写しが [`Slave`]。届いた変動で、自分の要素を書き換える（自動で同期する）。
//!   手元の利用者は通信を意識せず、[`Slave::get`] / [`Slave::snapshot`] で今の値を読み、
//!   [`Slave::changed`] で変動を待ち、[`Slave::set`] で Origin に置き換えを頼む
//! - Slave の合流の決まり: **同じ代で、手元より新しい rev だけを受け入れる**。
//!   rev は要素ごとに独立しているので、別々の道（stream、応答）から順不同で届いても矛盾しない
//! - Slave も Origin と同じ形の状態を持つので、そのまま下流の Slave への配り手（中継）になれる
//!   （[`Source`] / [`Relay`]）。rev は Origin が振ったものをそのまま運ぶ
//! - 配り手は、購読者ごとに「前回から変わった要素」だけを、今の値で渡す（途中の rev は畳む）。
//!   受け取りが遅い購読者がいても、溜まるのは要素の数まで

use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::ops::{Bound, Deref, DerefMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Semaphore, oneshot, watch};
use tokio::task::JoinHandle;

use crate::network::context::ConnectionContext;
use crate::network::server::ServerHandle;
use crate::network::{
    MessageType, NetworkError, ProtocolClient, ProtocolMessage, ProtocolServer, UnisonChannel,
    UnisonStream,
};

/// スキーマの正本（`schemas/sync.kdl`）。ビルドに焼き込み、discovery で配る
pub const SYNC_KDL: &str = include_str!("../../../../schemas/sync.kdl");

pub const PROTOCOL_NAME: &str = "unison-sync";
pub const NAMESPACE: &str = "club.chronista.unison.sync";
pub const CONTROL_CHANNEL: &str = "unison.sync";
pub const FEED_CHANNEL: &str = "unison.sync.feed";
pub const JOIN: &str = "Join";
pub const SET: &str = "Set";
pub const UPDATE: &str = "Update";

/// 入った後、配り手の側から feed が開かれるまで待つ既定の時間
pub const FEED_PATIENCE: Duration = Duration::from_secs(5);

/// 1 つの Slave（control チャネル 1 本）から同時に出せる、置き換えの頼みの数。
///
/// 配り手の側は、control に届いた request を待ち行列（`UnisonChannel`、256 件）から
/// 1 つずつ処理する。待ち行列が満ちると届いた request は黙って捨てられ、頼んだ側は
/// 応答を待ったまま時間切れになる。同時に出す数をそれより小さく抑え、超えたぶんは手元で待たせる
pub const MAX_SETS_IN_FLIGHT: usize = 128;

/// 要素 1 つの状態
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotState {
    /// 世代番号。Origin が確定するたびに 1 つ進む。最初の確定が 1
    pub rev: u64,
    pub value: Value,
}

/// 要素 1 つの変動。Origin が振った rev をそのまま運ぶ
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotUpdate {
    /// 何代めの Origin の変動か
    pub life: u64,
    pub slot: String,
    pub rev: u64,
    pub value: Value,
}

/// ある時点の状態を丸ごと
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// 何代めの Origin の状態か
    pub life: u64,
    pub slots: BTreeMap<String, SlotState>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    /// 配り手が止まった、または上流から外れた
    #[error("閉じている")]
    Closed,
    /// 上流が断った。中身は理由
    #[error("断られた: {0}")]
    Refused(String),
    /// 上流に届かなかった
    #[error("上流に届かない: {0}")]
    Unreachable(String),
}

// ── 状態（IO も同期の道具も持たない）──────────────────────

/// 要素の集まり。Origin と Slave が同じものを持ち、違うのは書き換え方
/// （[`Slots::set`] で確定する / [`Slots::merge`] で合流する）だけ
#[derive(Clone, Debug)]
pub struct Slots {
    life: u64,
    entries: BTreeMap<String, Entry>,
    /// 変わった順の索引（この Slots での通し番号 → 要素名）。
    /// 各要素は、最後に変わった 1 か所にだけいる
    order: BTreeMap<u64, String>,
    /// この Slots での通し番号。何かを受け入れるたびに 1 つ進む（rev とは別物）
    seq: u64,
}

#[derive(Clone, Debug)]
struct Entry {
    state: SlotState,
    seq: u64,
}

impl Slots {
    /// まだ何も置かれていない状態
    pub fn new(life: u64) -> Self {
        Self {
            life,
            entries: BTreeMap::new(),
            order: BTreeMap::new(),
            seq: 0,
        }
    }

    /// 丸ごとの状態から作る（Slave の始まり）
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        let mut slots = Self::new(snapshot.life);
        for (name, state) in snapshot.slots {
            slots.put(name, state);
        }
        slots
    }

    pub fn life(&self) -> u64 {
        self.life
    }

    /// この Slots での通し番号（[`Slots::changes_since`] に渡す）
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn get(&self, slot: &str) -> Option<&SlotState> {
        self.entries.get(slot).map(|entry| &entry.state)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            life: self.life,
            slots: self
                .entries
                .iter()
                .map(|(name, entry)| (name.clone(), entry.state.clone()))
                .collect(),
        }
    }

    /// 置き換える（Origin）: その要素の rev を 1 つ進めて確定する
    pub fn set(&mut self, slot: &str, value: Value) -> SlotUpdate {
        let rev = self.get(slot).map_or(0, |state| state.rev) + 1;
        self.put(
            slot.to_string(),
            SlotState {
                rev,
                value: value.clone(),
            },
        );
        SlotUpdate {
            life: self.life,
            slot: slot.to_string(),
            rev,
            value,
        }
    }

    /// 合流する（Slave）: 同じ代で、手元より新しい rev だけを受け入れる。受け入れたら `true`。
    /// 古い rev、同じ rev、別の代の変動は、何も変えない
    pub fn merge(&mut self, update: &SlotUpdate) -> bool {
        if update.life != self.life || update.rev == 0 {
            return false;
        }
        if self
            .get(&update.slot)
            .is_some_and(|state| state.rev >= update.rev)
        {
            return false;
        }
        self.put(
            update.slot.clone(),
            SlotState {
                rev: update.rev,
                value: update.value.clone(),
            },
        );
        true
    }

    /// 通し番号 `since` より後に変わった要素の、今の値（変わった順）。途中の rev は畳まれている
    pub fn changes_since(&self, since: u64) -> Vec<SlotUpdate> {
        self.order
            .range((Bound::Excluded(since), Bound::Unbounded))
            .map(|(_, name)| {
                let state = &self.entries[name].state;
                SlotUpdate {
                    life: self.life,
                    slot: name.clone(),
                    rev: state.rev,
                    value: state.value.clone(),
                }
            })
            .collect()
    }

    fn put(&mut self, name: String, state: SlotState) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(old) = self.entries.insert(name.clone(), Entry { state, seq }) {
            self.order.remove(&old.seq);
        }
        self.order.insert(seq, name);
    }
}

// ── 共有（Origin と Slave の中身）────────────────────────

/// 購読者への知らせ。中身は「どこまで変わったか」と「閉じたか」だけで、
/// 変わった要素そのものは、購読者が [`Slots`] から引く（だから溜まらない）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Version {
    seq: u64,
    closed: bool,
}

struct Shared {
    slots: Mutex<Slots>,
    version: watch::Sender<Version>,
    /// 閉じたことだけの知らせ。返事を待つ頼み（[`Link`]）が使う。変動では起きない
    closed: watch::Sender<bool>,
}

impl Shared {
    fn new(slots: Slots) -> Arc<Self> {
        let (version, _) = watch::channel(Version {
            seq: slots.seq(),
            closed: false,
        });
        let (closed, _) = watch::channel(false);
        Arc::new(Self {
            slots: Mutex::new(slots),
            version,
            closed,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Slots> {
        self.slots.lock().expect("slots の lock が壊れている")
    }

    /// 書き換えたことを購読者に知らせる。lock を握ったまま呼ぶので、
    /// 購読者が見る通し番号と中身は食い違わない
    fn publish(&self, slots: &Slots) {
        let seq = slots.seq();
        self.version.send_if_modified(|version| {
            let moved = version.seq != seq;
            version.seq = seq;
            moved
        });
    }

    fn close(&self) {
        let _slots = self.lock();
        self.version.send_if_modified(|version| {
            let was = version.closed;
            version.closed = true;
            !was
        });
        self.closed
            .send_if_modified(|closed| !std::mem::replace(closed, true));
    }

    fn is_closed(&self) -> bool {
        self.version.borrow().closed
    }

    /// 購読する。返すのは、購読した時点の状態と、それより後の変動の購読。
    /// 状態は購読の中に持たせない（入った人の数だけ写しを抱えないため）
    fn subscribe(self: &Arc<Self>) -> (Snapshot, Subscription) {
        let slots = self.lock();
        let version = self.version.subscribe();
        let subscription = Subscription {
            shared: self.clone(),
            version,
            cursor: slots.seq(),
        };
        (slots.snapshot(), subscription)
    }

    fn merge(&self, update: &SlotUpdate) -> bool {
        let mut slots = self.lock();
        let accepted = slots.merge(update);
        if accepted {
            self.publish(&slots);
        }
        accepted
    }
}

/// 配り手の変動の購読。購読した時点より後に変わった要素を受け取る
/// （購読した時点の状態は、購読するときに別に受け取る）
pub struct Subscription {
    shared: Arc<Shared>,
    version: watch::Receiver<Version>,
    /// どこまで渡したか（配り手の [`Slots`] での通し番号）
    cursor: u64,
}

impl Subscription {
    /// 次の変動を待つ。前回から変わった要素の、今の値を変わった順に返す（途中の rev は畳まれる）。
    /// 配り手が閉じたら、残りの変動を返したあとに `None`
    pub async fn next(&mut self) -> Option<Vec<SlotUpdate>> {
        loop {
            let (changes, closed) = {
                let slots = self.shared.lock();
                let changes = slots.changes_since(self.cursor);
                self.cursor = slots.seq();
                (changes, self.version.borrow_and_update().closed)
            };
            if !changes.is_empty() {
                return Some(changes);
            }
            if closed {
                return None;
            }
            self.version.changed().await.ok()?;
        }
    }
}

// ── Origin ─────────────────────────────────────────────

/// 正本。複製して配ってよい（中身は共有）
#[derive(Clone)]
pub struct Origin {
    shared: Arc<Shared>,
}

impl Origin {
    /// `life` 代めの Origin を、要素が空の状態で作る
    pub fn new(life: u64) -> Self {
        Self {
            shared: Shared::new(Slots::new(life)),
        }
    }

    pub fn life(&self) -> u64 {
        self.shared.lock().life()
    }

    pub fn get(&self, slot: &str) -> Option<SlotState> {
        self.shared.lock().get(slot).cloned()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().snapshot()
    }

    /// 置き換える: その要素の rev を 1 つ進めて確定し、購読者に知らせる。
    /// 空の要素名は `SyncError::Refused("bad_request")` で断る
    pub fn set(&self, slot: &str, value: Value) -> Result<SlotUpdate, SyncError> {
        let mut slots = self.shared.lock();
        if self.shared.is_closed() {
            return Err(SyncError::Closed);
        }
        if slot.is_empty() {
            return Err(SyncError::Refused("bad_request".into()));
        }
        let update = slots.set(slot, value);
        self.shared.publish(&slots);
        Ok(update)
    }

    /// 変動を購読する。購読した時点の状態と、以後の変動の購読を返す
    pub fn subscribe(&self) -> (Snapshot, Subscription) {
        self.shared.subscribe()
    }

    /// 閉じる（止まる）。購読者は、残りの変動を受け取ったあとに終わる。以後は置き換えられない
    pub fn close(&self) {
        self.shared.close();
    }

    pub fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }
}

// ── 上流と配り手 ─────────────────────────────────────────

/// 置き換えの頼みの返事を待つ future
pub type SetFuture<'a> = Pin<Box<dyn Future<Output = Result<SlotUpdate, SyncError>> + Send + 'a>>;

/// 上流（Origin の側）へ置き換えを頼む口。運び方ごとに実装する
pub trait Upstream: Send + Sync {
    /// 置き換えを頼み、Origin が確定した変動を返す
    fn set(&self, slot: String, value: Value) -> SetFuture<'_>;
}

impl Upstream for Origin {
    fn set(&self, slot: String, value: Value) -> SetFuture<'_> {
        Box::pin(std::future::ready(Origin::set(self, &slot, value)))
    }
}

/// 配り手。Origin も、Slave（中継）もなれる
pub trait Source {
    /// 変動を購読する。購読した時点の状態と、以後の変動の購読を返す
    fn subscribe(&self) -> (Snapshot, Subscription);
    /// 置き換えの頼みの行き先
    fn upstream(&self) -> Arc<dyn Upstream>;
}

impl Source for Origin {
    fn subscribe(&self) -> (Snapshot, Subscription) {
        Origin::subscribe(self)
    }

    fn upstream(&self) -> Arc<dyn Upstream> {
        Arc::new(self.clone())
    }
}

// ── Slave ──────────────────────────────────────────────

/// 手元の写し。届いた変動で、自分の要素を書き換える。
///
/// 手放す（drop する）と上流から外れる: 吸い上げる係が止まり、
/// この Slave を通して入っている下流の購読も終わる
pub struct Slave {
    link: Arc<Link>,
    /// 手元の利用者のための購読（[`Slave::changed`]）
    watch: Subscription,
    /// 生きている印。これが手放されると、[`Inlet::slave_gone`] が返る
    _alive: watch::Receiver<()>,
}

/// Slave の中身と、上流への口。中継では、下流からの頼みの行き先にもなる
struct Link {
    shared: Arc<Shared>,
    upstream: Arc<dyn Upstream>,
}

impl Upstream for Link {
    fn set(&self, slot: String, value: Value) -> SetFuture<'_> {
        Box::pin(async move {
            if self.shared.is_closed() {
                return Err(SyncError::Closed);
            }
            // 返事を待つ間に上流から外れたら、待つのをやめる。
            // 待つのは「閉じた」だけの知らせ。変動の知らせ（version）だと、変動のたびに起きてしまう
            let mut closed = self.shared.closed.subscribe();
            let confirmed = tokio::select! {
                biased;
                confirmed = self.upstream.set(slot, value) => confirmed?,
                _ = closed.wait_for(|closed| *closed) => return Err(SyncError::Closed),
            };
            // 応答の道でも合流させる。頼んだ側は、変動の知らせを待たずに確定した値を持つ
            self.shared.merge(&confirmed);
            Ok(confirmed)
        })
    }
}

/// 上流から届いた変動を Slave に流し込む口。運び方の受け取り係が持つ。
/// 手放すと、Slave は上流から外れたことになる（[`Inlet::close`] と同じ）
pub struct Inlet {
    link: Arc<Link>,
    alive: watch::Sender<()>,
}

impl Inlet {
    /// 届いた変動を合流させる。受け入れたら `true`（古い rev・別の代は捨てる）
    pub fn apply(&self, update: &SlotUpdate) -> bool {
        self.link.shared.merge(update)
    }

    /// 上流が切れた・止まった。Slave と、その下流の購読者に伝わる
    pub fn close(&self) {
        self.link.shared.close();
    }

    /// Slave が手放されるまで待つ（受け取り係が、自分の後始末に使う）
    pub async fn slave_gone(&self) {
        self.alive.closed().await;
    }
}

impl Drop for Inlet {
    fn drop(&mut self) {
        self.close();
    }
}

impl Slave {
    /// 初めの状態と、上流への口から Slave を作る。
    /// 返す [`Inlet`] に、上流から届いた変動を流し込む
    pub fn new(snapshot: Snapshot, upstream: Arc<dyn Upstream>) -> (Slave, Inlet) {
        let shared = Shared::new(Slots::from_snapshot(snapshot));
        let link = Arc::new(Link {
            shared: shared.clone(),
            upstream,
        });
        let (alive, alive_rx) = watch::channel(());
        let slave = Slave {
            link: link.clone(),
            watch: shared.subscribe().1,
            _alive: alive_rx,
        };
        (slave, Inlet { link, alive })
    }

    /// 同じプロセスの中の配り手（Origin、または中継する Slave）に入る。
    /// 吸い上げる係を裏で立てるので、tokio の中で呼ぶ
    pub fn follow(source: &impl Source) -> Slave {
        let (snapshot, mut subscription) = source.subscribe();
        let (slave, inlet) = Slave::new(snapshot, source.upstream());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    next = subscription.next() => match next {
                        Some(updates) => {
                            for update in &updates {
                                inlet.apply(update);
                            }
                        }
                        None => break,
                    },
                    _ = inlet.slave_gone() => break,
                }
            }
        });
        slave
    }

    /// 何代めの Origin の写しか
    pub fn life(&self) -> u64 {
        self.link.shared.lock().life()
    }

    /// 手元の、ある要素の今の値
    pub fn get(&self, slot: &str) -> Option<SlotState> {
        self.link.shared.lock().get(slot).cloned()
    }

    /// 手元の状態を丸ごと
    pub fn snapshot(&self) -> Snapshot {
        self.link.shared.lock().snapshot()
    }

    /// 手元の変動を待つ。前回から変わった要素の、今の値を返す（途中の rev は畳まれる）。
    /// 上流から外れたら、残りを返したあとに `None`
    pub async fn changed(&mut self) -> Option<Vec<SlotUpdate>> {
        self.watch.next().await
    }

    /// Origin に置き換えを頼む。確定した変動を返し、手元にも合流させる
    pub async fn set(&self, slot: &str, value: Value) -> Result<SlotUpdate, SyncError> {
        Upstream::set(&*self.link, slot.to_string(), value).await
    }

    /// 上流から外れたか
    pub fn is_closed(&self) -> bool {
        self.link.shared.is_closed()
    }
}

/// 中継: Slave は、そのまま下流の Slave への配り手になれる
impl Source for Slave {
    fn subscribe(&self) -> (Snapshot, Subscription) {
        self.link.shared.subscribe()
    }

    fn upstream(&self) -> Arc<dyn Upstream> {
        self.link.clone()
    }
}

// ── 中継ハンドル (Relay) ──────────────────────────────────

/// 上流に入っている写し（[`Slave`] / [`ConnectedSlave`]）を預かり、下流への配り手にするハンドル。
/// 複製して配ってよい。
///
/// 頼みを運ぶ間、lock は握らない。上流の返事を待っている頼みがあっても、
/// ほかの下流の「入る」「頼む」は待たされない。
#[derive(Clone)]
pub struct Relay {
    inner: Arc<RelayInner>,
}

struct RelayInner {
    life: u64,
    held: Mutex<Option<Held>>,
    visitors: Arc<AtomicUsize>,
}

struct Held {
    slave: Slave,
    pump: Option<JoinHandle<()>>,
}

/// 中継を通して入っている、ということ。手放す（drop する）と出ていく
pub struct RelayPresence {
    updates: Subscription,
    _visit: VisitGuard,
}

impl RelayPresence {
    /// 次の変動を待つ。前回から変わった要素の今の値を、変わった順に返す（途中の rev は畳まれる）。
    /// 中継が止まる・上流から外れると `None`
    pub async fn next(&mut self) -> Option<Vec<SlotUpdate>> {
        self.updates.next().await
    }
}

struct VisitGuard(Arc<AtomicUsize>);

impl Drop for VisitGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Relay {
    /// QUIC で上流に入っている写しを預かる
    pub fn new(slave: ConnectedSlave) -> Self {
        let (slave, pump) = slave.into_parts();
        Self::hold(slave, Some(pump))
    }

    /// 同じプロセス内の写しを預かる
    pub fn from_slave(slave: Slave) -> Self {
        Self::hold(slave, None)
    }

    /// 写しと後始末タスクを預かる
    pub fn hold(slave: Slave, pump: Option<JoinHandle<()>>) -> Self {
        let life = slave.life();
        Self {
            inner: Arc::new(RelayInner {
                life,
                held: Mutex::new(Some(Held { slave, pump })),
                visitors: Arc::new(AtomicUsize::new(0)),
            }),
        }
    }

    /// 中継している Origin の代
    pub fn life(&self) -> u64 {
        self.inner.life
    }

    /// 下流が入る。中継の手元の状態と、入っていること（以後の変動）を渡す。
    /// 止めた・上流から外れた中継には入れない
    pub fn join(&self) -> Result<(Snapshot, RelayPresence), SyncError> {
        let held = self.inner.lock();
        let held = held.as_ref().ok_or(SyncError::Closed)?;
        if held.slave.is_closed() {
            return Err(SyncError::Closed);
        }
        let (state, updates) = Source::subscribe(&held.slave);
        self.inner.visitors.fetch_add(1, Ordering::Relaxed);
        let presence = RelayPresence {
            updates,
            _visit: VisitGuard(self.inner.visitors.clone()),
        };
        Ok((state, presence))
    }

    /// 下流からの置き換えの頼みを、上流へ運ぶ。上流で確定した変動を返し、中継の手元にも合流させる
    pub async fn set(
        &self,
        slot: impl Into<String>,
        value: Value,
    ) -> Result<SlotUpdate, SyncError> {
        let slot = slot.into();
        if slot.is_empty() {
            return Err(SyncError::Refused("bad_request".into()));
        }
        // 上流への口だけを取り出し、lock を手放してから待つ
        let upstream = {
            let held = self.inner.lock();
            match held.as_ref() {
                Some(held) if !held.slave.is_closed() => Source::upstream(&held.slave),
                _ => return Err(SyncError::Closed),
            }
        };
        upstream.set(slot, value).await
    }

    /// 中継の手元の状態。止めていれば `None`
    pub fn snapshot(&self) -> Option<Snapshot> {
        let held = self.inner.lock();
        Some(held.as_ref()?.slave.snapshot())
    }

    /// 入らずに見る（入っている人には数えない）。止めていれば `None`
    pub fn observe(&self) -> Option<Subscription> {
        let held = self.inner.lock();
        Some(Source::subscribe(&held.as_ref()?.slave).1)
    }

    /// 中継を通して入っている人の数
    pub fn visitors(&self) -> usize {
        self.inner.visitors.load(Ordering::Relaxed)
    }

    /// 止めた、または上流から外れたか
    pub fn is_stopped(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_none_or(|held| held.slave.is_closed())
    }

    /// 止める: 写しを手放して上流から出ていく（つながりを閉じ終えるまで待つ）。
    /// 中継を通して入っていた人は全員外れる。すでに止めていれば何もしない
    pub async fn stop(&self) {
        let held = self.inner.lock().take();
        if let Some(Held { slave, pump }) = held {
            drop(slave);
            if let Some(pump) = pump {
                let _ = pump.await;
            }
        }
    }
}

impl RelayInner {
    fn lock(&self) -> MutexGuard<'_, Option<Held>> {
        self.held.lock().expect("中継の lock が壊れている")
    }
}

// ── QUIC 2 チャネル運び方（control + server 起点 feed）──────

/// 標準の `Join` request ペイロード
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JoinRequest {}

/// 標準の `Join` 応答。入れたら状態、入れなかったら理由
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JoinReply {
    Joined(Snapshot),
    Refused { error: String },
}

/// `Set` の中身
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SetRequest {
    pub slot: String,
    pub value: Value,
}

/// `Set` の応答。確定したら変動、しなかったら理由
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SetReply {
    Set(SlotUpdate),
    Refused { error: String },
}

#[derive(Debug, thiserror::Error)]
pub enum JoinError {
    /// 配り手の側が断った。中身は理由
    #[error("入れなかった: {0}")]
    Refused(String),
    #[error("つながらなかった: {0}")]
    Network(String),
}

/// server 起点 `feed` へ書き出す変動の源。手放す（drop する）と出ていく
pub trait FeedSource: Send + 'static {
    fn next(&mut self) -> impl Future<Output = Option<Vec<SlotUpdate>>> + Send;
}

impl FeedSource for Subscription {
    fn next(&mut self) -> impl Future<Output = Option<Vec<SlotUpdate>>> + Send {
        Subscription::next(self)
    }
}

impl FeedSource for RelayPresence {
    fn next(&mut self) -> impl Future<Output = Option<Vec<SlotUpdate>>> + Send {
        RelayPresence::next(self)
    }
}

/// QUIC の sync チャネルで待ち受けられる配り手（[`Origin`] または [`Relay`]）
pub trait SyncTarget: Send + Sync + 'static {
    type Presence: FeedSource;
    fn join(&self)
    -> impl Future<Output = Result<(Snapshot, Self::Presence), &'static str>> + Send;
    fn set(
        &self,
        slot: String,
        value: Value,
    ) -> impl Future<Output = Result<SlotUpdate, SyncError>> + Send;
}

impl SyncTarget for Origin {
    type Presence = Subscription;

    async fn join(&self) -> Result<(Snapshot, Self::Presence), &'static str> {
        if self.is_closed() {
            return Err("stopped");
        }
        Ok(self.subscribe())
    }

    async fn set(&self, slot: String, value: Value) -> Result<SlotUpdate, SyncError> {
        if slot.is_empty() {
            return Err(SyncError::Refused("bad_request".into()));
        }
        Origin::set(self, &slot, value)
    }
}

impl SyncTarget for Relay {
    type Presence = RelayPresence;

    async fn join(&self) -> Result<(Snapshot, Self::Presence), &'static str> {
        Relay::join(self).map_err(|_| "stopped")
    }

    async fn set(&self, slot: String, value: Value) -> Result<SlotUpdate, SyncError> {
        Relay::set(self, slot, value).await
    }
}

/// 配り手の変動を、入っている人の server 起点 `feed` へ書き続けるタスクを起動する。
///
/// 配り手（または中継・上流）が止まったとき、または `feed` に書けなくなったとき
/// （相手が `feed` を受け取っていない、切れた）は、先に `updates` を手放して退室させてから
/// `feed` と `channel`（control）を閉じて終わる。
pub fn spawn_feed<F: FeedSource>(
    mut updates: F,
    feed: UnisonStream,
    channel: Arc<UnisonChannel>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        'feed: while let Some(batch) = updates.next().await {
            for update in &batch {
                let frame = serde_json::to_value(update).and_then(|payload| {
                    ProtocolMessage::new_with_json(
                        0,
                        UPDATE.to_string(),
                        MessageType::Event,
                        payload,
                    )
                    .map_err(serde::ser::Error::custom)
                });
                let Ok(frame) = frame else {
                    tracing::warn!("Update を組み立てられない: {update:?}");
                    continue;
                };
                if let Err(error) = feed.send_frame(&frame).await {
                    tracing::debug!("feed に書けない。出ていったことにする: {error}");
                    break 'feed;
                }
            }
        }
        // 先に出ていく（入っている人に数えない）。それから、相手に伝わるよう、こちらから閉じる
        drop(updates);
        let _ = feed.close_stream().await;
        let _ = channel.close().await;
    })
}

/// `Set` request メッセージを検証して取り出す（空の要素名は `"bad_request"` で断る）
pub fn parse_set_request(message: &ProtocolMessage) -> Result<SetRequest, &'static str> {
    let request: SetRequest = message
        .payload_as_value()
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or("bad_request")?;
    if request.slot.is_empty() {
        return Err("bad_request");
    }
    Ok(request)
}

/// `SyncError` を wire 上の `SetReply` に変換する
pub fn into_set_reply(result: Result<SlotUpdate, SyncError>) -> SetReply {
    match result {
        Ok(update) => SetReply::Set(update),
        Err(SyncError::Refused(error)) => SetReply::Refused { error },
        Err(SyncError::Closed) => SetReply::Refused {
            error: "stopped".into(),
        },
        Err(SyncError::Unreachable(detail)) => {
            tracing::warn!("中継: 上流に届かない: {detail}");
            SetReply::Refused {
                error: "unreachable".into(),
            }
        }
    }
}

/// control チャネル 1 本ぶんの応対
pub async fn serve_sync_channel<T: SyncTarget>(
    target: Arc<T>,
    ctx: Arc<ConnectionContext>,
    channel: Arc<UnisonChannel>,
    feed_channel: &'static str,
) -> Result<(), NetworkError> {
    let mut joined: Option<JoinHandle<()>> = None;

    let result = loop {
        let message = match channel.recv().await {
            Ok(message) if message.msg_type == MessageType::Request => message,
            Ok(_) => continue,
            Err(error) if error.is_normal_close() => break Ok(()),
            Err(error) => break Err(error),
        };
        let sent = match message.method.as_str() {
            JOIN => {
                let reply = if joined.is_some() {
                    JoinReply::Refused {
                        error: "already_joined".into(),
                    }
                } else {
                    match target.join().await {
                        Ok((snapshot, presence)) => {
                            match ctx.open_server_stream(feed_channel).await {
                                Ok(feed) => {
                                    joined = Some(spawn_feed(presence, feed, channel.clone()));
                                    JoinReply::Joined(snapshot)
                                }
                                Err(error) => {
                                    tracing::warn!("feed を開けない: {error}");
                                    JoinReply::Refused {
                                        error: "feed_unavailable".into(),
                                    }
                                }
                            }
                        }
                        Err(reason) => JoinReply::Refused {
                            error: reason.into(),
                        },
                    }
                };
                channel
                    .send_response(message.id, &message.method, &reply)
                    .await
            }
            SET => {
                let reply = if joined.is_none() {
                    SetReply::Refused {
                        error: "not_joined".into(),
                    }
                } else {
                    match parse_set_request(&message) {
                        Ok(request) => {
                            into_set_reply(target.set(request.slot, request.value).await)
                        }
                        Err(reason) => SetReply::Refused {
                            error: reason.into(),
                        },
                    }
                };
                channel
                    .send_response(message.id, &message.method, &reply)
                    .await
            }
            other => {
                tracing::warn!("知らない request: {other}");
                channel
                    .send_error(
                        message.id,
                        &message.method,
                        &serde_json::json!({ "error": "unknown_request" }),
                    )
                    .await
            }
        };
        if let Err(error) = sent {
            break Err(error);
        }
    };

    if let Some(forwarder) = joined.take() {
        forwarder.abort();
    }
    result
}

/// 既定のチャネル名（[`CONTROL_CHANNEL`] / [`FEED_CHANNEL`]）で `ProtocolServer` に sync ハンドラを登録する
pub async fn register_sync_channel<T: SyncTarget>(server: &ProtocolServer, target: Arc<T>) {
    register_sync_channel_with(server, CONTROL_CHANNEL, FEED_CHANNEL, target).await;
}

/// チャネル名を指定して `ProtocolServer` に sync ハンドラを登録する
pub async fn register_sync_channel_with<T: SyncTarget>(
    server: &ProtocolServer,
    control_channel: &'static str,
    feed_channel: &'static str,
    target: Arc<T>,
) {
    server
        .register_channel(control_channel, move |ctx, stream| {
            let target = target.clone();
            async move {
                serve_sync_channel(
                    target,
                    ctx,
                    Arc::new(UnisonChannel::new(stream)),
                    feed_channel,
                )
                .await
            }
        })
        .await;
}

/// 単一の配り手（[`Origin`] または [`Relay`]）を QUIC で待ち受けるサーバ
pub struct SyncServer {
    handle: ServerHandle,
}

impl SyncServer {
    /// 待ち受けを始める
    pub async fn start<T: SyncTarget>(target: Arc<T>, listen: &str) -> anyhow::Result<Self> {
        let server =
            ProtocolServer::with_identity(PROTOCOL_NAME, env!("CARGO_PKG_VERSION"), NAMESPACE);
        server.enable_discovery(SYNC_KDL).await?;
        register_sync_channel(&server, target).await;
        let handle = Arc::new(server).listener(listen).spawn().await?;
        Ok(Self { handle })
    }

    /// 実際に待ち受けているアドレス
    pub fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// 待ち受けをやめる
    pub async fn shutdown(self) {
        if let Err(error) = self.handle.shutdown().await {
            tracing::warn!("待ち受けの終了で問題: {error}");
        }
    }
}

/// 配り手の側から開かれる server 起点 `feed` stream の受け口。
/// クライアントが `connect` する前に [`FeedAcceptor::register`] で登録しておく。
pub struct FeedAcceptor {
    rx: oneshot::Receiver<UnisonStream>,
}

impl FeedAcceptor {
    /// `client` に server 起点 `feed_channel` の受け口を登録する
    pub async fn register(client: &ProtocolClient, feed_channel: &str) -> Self {
        let (feed_tx, feed_rx) = oneshot::channel::<UnisonStream>();
        let feed_tx = Arc::new(Mutex::new(Some(feed_tx)));
        client
            .register_server_channel(feed_channel, move |stream| {
                let feed_tx = feed_tx.clone();
                async move {
                    let sender = feed_tx.lock().expect("feed の受け口が壊れている").take();
                    match sender {
                        Some(sender) => {
                            let _ = sender.send(stream);
                        }
                        None => tracing::warn!("feed が 2 本目に開かれた。捨てる"),
                    }
                    Ok(())
                }
            })
            .await;
        Self { rx: feed_rx }
    }

    /// 配り手の側から `feed` が開かれるのを待つ
    pub async fn wait(self, timeout: Duration) -> Result<UnisonStream, JoinError> {
        match tokio::time::timeout(timeout, self.rx).await {
            Ok(Ok(feed)) => Ok(feed),
            _ => Err(JoinError::Network("feed が開かれない".into())),
        }
    }
}

/// 置き換えの頼みを、control チャネルの `Set` で配り手へ運ぶ
struct QuicUpstream {
    channel: Arc<UnisonChannel>,
    /// 同時に出している頼みの数を抑える（[`MAX_SETS_IN_FLIGHT`]）
    in_flight: Semaphore,
}

impl Upstream for QuicUpstream {
    fn set(&self, slot: String, value: Value) -> SetFuture<'_> {
        Box::pin(async move {
            let _permit = self
                .in_flight
                .acquire()
                .await
                .map_err(|_| SyncError::Closed)?;
            let reply: SetReply = self
                .channel
                .request(SET, &SetRequest { slot, value })
                .await
                .map_err(|error| SyncError::Unreachable(error.to_string()))?;
            match reply {
                SetReply::Set(update) => Ok(update),
                SetReply::Refused { error } => Err(SyncError::Refused(error)),
            }
        })
    }
}

/// `feed` を吸い上げて `Slave` に流し込む係。
///
/// 配り手が止まる・切れると、`Slave` に「外れた」を伝えて終わる。
/// `Slave` が手放されると、つながりを閉じて終わる（= 出ていく）
async fn pump(
    feed: UnisonStream,
    inlet: Inlet,
    channel: Arc<UnisonChannel>,
    client: ProtocolClient,
) {
    loop {
        tokio::select! {
            frame = feed.recv_frame() => match frame {
                Ok(message) if message.msg_type == MessageType::Event && message.method == UPDATE => {
                    let update = message
                        .payload_as_value()
                        .ok()
                        .and_then(|value| serde_json::from_value::<SlotUpdate>(value).ok());
                    match update {
                        Some(update) => {
                            inlet.apply(&update);
                        }
                        None => tracing::warn!("読めない Update を捨てた"),
                    }
                }
                Ok(_) => continue,
                Err(_) => break,
            },
            _ = inlet.slave_gone() => break,
        }
    }
    inlet.close();
    let _ = channel.close().await;
    let _ = client.disconnect().await;
}

/// QUIC で上流の配り手に入っている、手元の写し。
///
/// [`Slave`] として読める（`get` / `snapshot` / `changed` / `set`、中継の `Source`）。
/// 手放す（drop する）か [`ConnectedSlave::leave`] を呼ぶと出ていく。
pub struct ConnectedSlave {
    slave: Slave,
    /// `feed` を吸い上げる係
    pump: JoinHandle<()>,
}

impl ConnectedSlave {
    /// 取得済みの初期 `Snapshot`、control チャネル、server 起点 `feed` stream、`ProtocolClient` から
    /// 手元の [`ConnectedSlave`] を組み立て、裏で `feed` の吸い上げを始める
    pub fn attach(
        snapshot: Snapshot,
        channel: Arc<UnisonChannel>,
        feed: UnisonStream,
        client: ProtocolClient,
    ) -> Self {
        let upstream = Arc::new(QuicUpstream {
            channel: channel.clone(),
            in_flight: Semaphore::new(MAX_SETS_IN_FLIGHT),
        });
        let (slave, inlet) = Slave::new(snapshot, upstream);
        let pump = tokio::spawn(pump(feed, inlet, channel, client));
        Self { slave, pump }
    }

    /// 既定のチャネル名（[`CONTROL_CHANNEL`] / [`FEED_CHANNEL`]）で `addr` の配り手に入る
    pub async fn join(addr: SocketAddr) -> Result<Self, JoinError> {
        Self::join_with(addr, CONTROL_CHANNEL, FEED_CHANNEL).await
    }

    /// チャネル名を指定して `addr` の配り手に入る
    pub async fn join_with(
        addr: SocketAddr,
        control_channel: &str,
        feed_channel: &str,
    ) -> Result<Self, JoinError> {
        let network = |error: &dyn std::fmt::Display| JoinError::Network(error.to_string());

        let client = ProtocolClient::insecure_localhost().map_err(|e| network(&e))?;
        let acceptor = FeedAcceptor::register(&client, feed_channel).await;

        let entered = async {
            client
                .connect(&addr.to_string())
                .await
                .map_err(|e| network(&e))?;
            let channel = Arc::new(
                client
                    .open_channel(control_channel)
                    .await
                    .map_err(|e| network(&e))?,
            );
            let reply: JoinReply = channel
                .request(JOIN, &JoinRequest::default())
                .await
                .map_err(|e| network(&e))?;
            let snapshot = match reply {
                JoinReply::Joined(snapshot) => snapshot,
                JoinReply::Refused { error } => return Err(JoinError::Refused(error)),
            };
            let feed = acceptor.wait(FEED_PATIENCE).await?;
            Ok((snapshot, channel, feed))
        }
        .await;

        let (snapshot, channel, feed) = match entered {
            Ok(ok) => ok,
            Err(error) => {
                let _ = client.disconnect().await;
                return Err(error);
            }
        };

        Ok(Self::attach(snapshot, channel, feed, client))
    }

    /// 出ていく。つながりを閉じ終えるまで待つ
    pub async fn leave(self) {
        let Self { slave, pump } = self;
        drop(slave);
        let _ = pump.await;
    }

    /// 写しと吸い上げる係に分ける（[`Relay`] に預けるとき）。
    /// 写しを手放すと出ていき、吸い上げる係はつながりを閉じ終えたら終わる
    pub fn into_parts(self) -> (Slave, JoinHandle<()>) {
        (self.slave, self.pump)
    }
}

impl Deref for ConnectedSlave {
    type Target = Slave;

    fn deref(&self) -> &Slave {
        &self.slave
    }
}

impl DerefMut for ConnectedSlave {
    fn deref_mut(&mut self) -> &mut Slave {
        &mut self.slave
    }
}
