# State Sync 設計（`unison::sync`）

- 対応仕様: [`spec/05-sync/SPEC.md`](../spec/05-sync/SPEC.md)
- スキーマ: [`schemas/sync.kdl`](../schemas/sync.kdl)
- 実装: [`crates/unison-protocol/src/sync/mod.rs`](../crates/unison-protocol/src/sync/mod.rs)
- テスト:
  - Small: [`crates/unison-protocol/tests/test_small_sync.rs`](../crates/unison-protocol/tests/test_small_sync.rs)
  - Medium (実 QUIC): [`crates/unison-protocol/tests/test_medium_sync.rs`](../crates/unison-protocol/tests/test_medium_sync.rs)

---

## 1. アーキテクチャと層分け

`unison::sync` は、**IO を持たない同期コア**、**状態と上流を預かる汎用中継（`Relay`）**、**QUIC 2 チャネル運び方** の 3 層で構成される。

```
┌────────────────────────────────────────────────────────────────┐
│ 利用側（単一 Origin/Relay の SyncServer、または field 等の Host）│
├────────────────────────────────────────────────────────────────┤
│ QUIC 2 チャネル運び方                                           │
│  Server 側: SyncServer / register_sync_channel / spawn_feed     │
│  Client 側: ConnectedSlave / FeedAcceptor (Semaphore 128)      │
├────────────────────────────────────────────────────────────────┤
│ 中継 (Relay / RelayPresence)                                    │
│  上流の Slave を預かり、lock を握らずに下流の Join / Set を捌く  │
├────────────────────────────────────────────────────────────────┤
│ 同期コア (Slots / Origin / Subscription / Slave / Inlet / Source)│
│  IO なし。要素ごとの (rev, value)、最新値への畳み込み、合流規則   │
└────────────────────────────────────────────────────────────────┘
```

| 型 | 役割 | IO |
|---|---|---|
| `Slots` | 要素の集まり。`set` で確定（Origin）、`merge` で合流（Slave）、`changes_since` で「前回より後に変わった要素の最新値」を引く | なし |
| `Origin` | 正本。`set` / `get` / `snapshot` / `subscribe` / `close`。`subscribe` は `(Snapshot, Subscription)` を返し余計な写しを抱えない | なし |
| `Subscription` | 購読。`next()` で前回から変わった要素の今の値を変わった順に返す（途中の `rev` は畳まれる） | なし |
| `Slave` | 手元の写し。`get` / `snapshot` / `changed` / `set`。drop で上流から外れ、返事待ちの `set` も即座に `SyncError::Closed` で終わる | なし |
| `Inlet` | 運び方の受け取り係が、届いた変動を `Slave` に流し込む口（`apply` / `close` / `slave_gone`） | なし |
| `Upstream` / `Source` | 置き換えの頼みの行き先（`Upstream`）と配り手（`Source` = `subscribe` + `upstream`）。`Origin` も `Slave` も実装する | なし |
| `Relay` / `RelayPresence` | 上流に繋がる `Slave` を預かり、訪問者数管理・非ブロッキングな `Set` 中継・`stop` 待機を行う中継ハンドル | なし |
| `SyncServer` / `ConnectedSlave` | `unison.sync`（client 起点）+ `unison.sync.feed`（server 起点）の 2 チャネルで `Origin` / `Relay` と `Slave` を QUIC 接続する | QUIC |

---

## 2. 主要な設計判断

### 2.1 購読者ごとの queue を持たない「新しいものだけ」の畳み込み

- `Slots` は `entries: BTreeMap<String, Entry>` と、変更順の索引 `order: BTreeMap<u64, String>`（この `Slots` 内の通し番号 `seq` → 要素名）を持つ。
- 要素が更新されると、古い `seq` の索引を消して新しい `seq` に差し替えるため、`order` の要素数は常に **要素（slot）の種類数以下** に保たれる。
- `Shared` は `tokio::sync::watch::Sender<Version>` で最新の `seq` と `closed` だけを知らせ、各 `Subscription` は自分の `cursor: u64` を使って `slots.changes_since(cursor)` を引く。
- これにより、購読者ごとのメッセージ queue が存在せず、遅い購読者がいてもメモリ滞留は要素数で頭打ちになり、`stream` に書き込む前に古い `rev` が自然に畳まれる。

### 2.2 道を 2 本に分ける（client 起点 control + server 起点 feed）

- client 起点の `UnisonChannel` は event 受信に 256 件の固定長 queue（`try_deliver`）を使い、満杯時に黙って捨てる。要素単位の `Update` が 1 件でも捨てられると、その要素が次に更新されるまで写しが正本と食い違う。
- そのため、`Join` と `Set`（request/response）は client 起点の control チャネル（既定 `unison.sync`）で送り、変動の配信（`Update` event）は server 起点の raw `UnisonStream`（既定 `unison.sync.feed`、`ctx.open_server_stream` + `client.register_server_channel`）で直接読み書きする。
- 入るときの順序:
  1. Client は接続前に `FeedAcceptor::register(&client, FEED_CHANNEL)` で server 起点 stream の受け口を置く。
  2. Client が接続して control チャネルを開き、`Join` を送る。
  3. Server は `target.join()` で `(Snapshot, Presence)` を得て、**`Join` の応答を返す前に** `ctx.open_server_stream(FEED_CHANNEL)` を開き、`spawn_feed` で変動の書き出しタスクを起動してから `JoinReply::Joined(snapshot)` を返す。
  4. `Update` が `JoinReply` より先に届くことがあっても、`Slave` は `Snapshot` から初期化された後に `feed` を吸い上げ、`Slots::merge`（同じ `life` で手元より新しい `rev` のみ採用）で合流するため矛盾しない。

### 2.3 同時 in-flight `Set` の抑制（`MAX_SETS_IN_FLIGHT = 128`）

- Server 側の `UnisonChannel` は、届いた **request** も event と同じ 256 件の待ち行列（`try_deliver`）に通す。1 つの `Slave`（または下流の頼みを束ねる `Relay`）から一度に大量の `Set` を送ると、256 件を超えた分が黙って捨てられ、呼び出し側が 30 秒の request timeout まで待たされる。
- これを防ぐため、Client 側の `QuicUpstream` は `Semaphore::new(128)`（`MAX_SETS_IN_FLIGHT`）で 1 チャネルあたりの同時 in-flight `Set` 数を 128 以下に抑え、超えた分は手元で順番待ちさせる（3,000 並列 `Set` でも取りこぼしゼロで完走する）。

### 2.4 上流から外れたら返事待ちの `Set` を即座に終わらせる（専用 `closed` watch）

- `UnisonChannel::close()` は受信ループと stream を閉じるが、すでに送信して返事待ちの `request()`（`pending` map）を即座には解かず、30 秒の timeout まで待たせる。
- 上流が止まった・切れたときや `Relay::stop()` したときに返事待ちの `Slave::set` タスクが残らないよう、`Slave` の `Link::set` は上流への `upstream.set` と `self.shared.closed`（閉じたことだけを知らせる専用 `watch::Receiver<bool>`）を `tokio::select!` する。
- **注意**: 変動の知らせ（`version: watch::Sender<Version>`）と同じ watch で閉じを待つと、変動が 1 件合流するたびに返事待ちの数千個の `Set` future がすべて起床（poll）してしまう（3,000 並列 `Set` で 0.35s → 1.1s に悪化）。そのため「閉じた」だけの `watch::Sender<bool>` を分離している（`pending_set_is_not_woken_by_updates` テストで保証）。

### 2.5 `Relay` は上流の返事を待つ間に lock を握らない

- `Relay::set` は `Mutex` を握って `Source::upstream(&held.slave)`（`Arc<dyn Upstream>`）を取り出した直後に lock を手放し、その後に `upstream.set(slot, value).await` を待つ。
- これにより、ある下流の `Set` が上流の返事を待っていても、他の下流の `Join` や別の `Set` は一切ブロックされない。
- また、1 本の control チャネル内では request を受信順に 1 つずつ処理するため、同一クライアント内の `Set` の順序は保たれる。

---

## 3. カスタムプロトコル（複数ターゲットの Host 等）との組み合わせ

単一の `Origin` や `Relay` をそのまま公開する場合は `SyncServer::start(target, listen)` / `ConnectedSlave::join(addr)` で完結する。

一方、`club-field` のように 1 つのサーバー（`Host`）で複数の名前付きターゲットを管理し、独自の `Join` ペイロード（`{ owner, name }` 等）と拡張スナップショットをやり取りする場合も、`unison::sync` の部品をそのまま合成できる：

- **Server 側**:
  - 独自の `Join` ハンドラでターゲットを解決し、`ctx.open_server_stream(feed_channel)` を開いた後、`unison::sync::spawn_feed(presence, feed_stream, control_channel)` で feed 配信タスクを起動する。
  - `Set` ハンドラは `unison::sync::parse_set_request(&message)` と `unison::sync::into_set_reply(result)`（および `SetRequest` / `SetReply`）をそのまま使う。
- **Client 側**:
  - 接続前に `let acceptor = FeedAcceptor::register(&client, feed_channel).await;` を置く。
  - 独自の `Join` request で初期 `Snapshot` を受け取り、`let feed = acceptor.wait(FEED_PATIENCE).await?;` で server 起点 stream を受け取る。
  - `ConnectedSlave::attach(snapshot, control_channel, feed, client)` を呼ぶだけで、`Semaphore(128)` 付きの `QuicUpstream` と `pump` タスクが立ち上がる。
- **Relay 側**:
  - `unison::sync::Relay` を内部に持ち、必要なら名札（`FieldInfo` 等）を横に添えるだけで多段中継が成立する。
