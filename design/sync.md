# State Sync 設計（`unison::sync`）

- 対応仕様: [`spec/05-sync/SPEC.md`](../spec/05-sync/SPEC.md)
- Protobuf 定義: [`crates/unison-protocol/proto/sync.proto`](../crates/unison-protocol/proto/sync.proto)
- KDL スキーマ: [`schemas/sync.kdl`](../schemas/sync.kdl)
- 実装: [`crates/unison-protocol/src/sync/mod.rs`](../crates/unison-protocol/src/sync/mod.rs)
- テスト:
  - Small: [`crates/unison-protocol/tests/test_small_sync.rs`](../crates/unison-protocol/tests/test_small_sync.rs)
  - Medium (実 QUIC): [`crates/unison-protocol/tests/test_medium_sync.rs`](../crates/unison-protocol/tests/test_medium_sync.rs)

---

## 1. アーキテクチャと層分け

`unison::sync` は、**IO を持たない同期コア（`bytes::Bytes` 正本）**、**状態と上流を預かる汎用中継（`Relay`）**、**`buffa` バイナリによる QUIC 2 チャネル運び方** の 3 層で構成される。

```
┌────────────────────────────────────────────────────────────────┐
│ 利用側（buffa MessageView による no-alloc 読み取り / Proto / JSON）│
├────────────────────────────────────────────────────────────────┤
│ QUIC 2 チャネル運び方 (proto/sync.proto — buffa binary wire)     │
│  Server 側: SyncServer / register_sync_channel / spawn_feed     │
│  Client 側: ConnectedSlave / FeedAcceptor (Semaphore 128)      │
├────────────────────────────────────────────────────────────────┤
│ 中継 (Relay / RelayPresence)                                    │
│  上流の Slave を預かり、lock を握らずに下流の Join / Set を捌く  │
├────────────────────────────────────────────────────────────────┤
│ 同期コア (Slots / Origin / Subscription / Slave / Inlet / Source)│
│  IO なし。要素ごとの (rev, Bytes)、O(1) 複製、最新値への畳み込み │
└────────────────────────────────────────────────────────────────┘
```

| 型 | 役割 | IO |
|---|---|---|
| `SlotState` / `SlotUpdate` | 要素 1 つの状態 `{ rev, payload: Bytes }` と変動 `{ life, slot, rev, payload: Bytes }`。`decode_view::<V>()` で **no-alloc 読み取り** を提供 | なし |
| `Slots` | 要素の集まり。`set_bytes` / `set_proto` / `set` で確定（Origin）、`merge` で合流（Slave）、`changes_since` で「前回より後に変わった要素の最新値」を引く | なし |
| `Origin` | 正本。`set_proto` / `set_bytes` / `set_json` / `set`、`with_slot` / `get` / `get_owned_view` / `get_proto` / `get_json` / `snapshot` / `subscribe` / `close` | なし |
| `Subscription` | 購読。`next()` で前回から変わった要素の今の値を変わった順に返す（途中の `rev` は畳まれる） | なし |
| `Slave` | 手元の写し。`with_slot` / `get` / `get_owned_view` / `get_proto` / `get_json` / `snapshot` / `changed`、`set_proto` / `set_bytes` / `set_json` / `set` | なし |
| `Inlet` | 運び方の受け取り係が、届いた変動を `Slave` に流し込む口（`apply` / `close` / `slave_gone`） | なし |
| `Upstream` / `Source` | 置き換えの頼みの行き先（`Upstream::set_bytes`）と配り手（`Source` = `subscribe` + `upstream`）。`Origin` も `Slave` も実装する | なし |
| `Relay` / `RelayPresence` | 上流に繋がる `Slave` を預かり、訪問者数管理・非ブロッキングな `Set` 中継・`stop` 待機を行う中継ハンドル | なし |
| `SyncServer` / `ConnectedSlave` | `unison.sync`（client 起点）+ `unison.sync.feed`（server 起点）の 2 チャネルで `Origin` / `Relay` と `Slave` を QUIC 接続する | QUIC |

---

## 2. 主要な設計判断

### 2.1 `bytes::Bytes` 正本と `buffa::MessageView<'a>` による完全 no-alloc 読み取り

- **なぜ `serde_json::Value` ではなく `bytes::Bytes` を正本にするか**:
  - `Value` を正本にすると、`Origin` → `Subscription` → `Relay` → `Slave::get` の各段階で JSON 木の深コピー（ヒープアロケーション）が発生し、wire 送受信のたびに JSON のシリアライズ・デシリアライズが走る。
  - `SlotState.payload` と `SlotUpdate.payload` を `bytes::Bytes` に統一することで、すべての複製が **O(1)（参照カウントの増減のみ、ヒープ確保ゼロ）** になり、`Origin` から何段 `Relay` を経由して `Slave` に届くまでも再エンコードが一切起きない。
- **3 通りの no-alloc 読み取りパス**:
  1. **`slave.with_slot("pose", |state| state.decode_view::<PoseView>())`**:
     `Mutex` ロック内で `&SlotState` の `&[u8]` スライスから直接 `V::decode_view` を実行する。`Bytes::clone()` の参照カウント操作すら行わず、**ヒープ確保 0 回（no-alloc）**。
  2. **`let state = slave.get("pose")?; let view = state.decode_view::<PoseView>()?;`**:
     `slave.get` は `Bytes::clone()` のみでロックを即座に手放し、返った `SlotState` の `&[u8]` から `V::decode_view` する。これも **ヒープ確保 0 回（no-alloc）**。
  3. **`slave.get_owned_view::<PoseView<'static>>("pose")`**:
     `buffa::OwnedView<V>`（`Bytes` バッファと zero-copy View を対にした `'static + Send + Sync` ハンドル）を **ヒープ確保 0 回** で返す。`test_small_sync.rs` の `CountingAllocator` により、これら 3 パスすべてで `alloc_count == 0` を実測検証している。
- **Wire の `buffa` バイナリ化と JSON フォールバック**:
  - `crates/unison-protocol/proto/sync.proto` で `SlotEntryProto` / `SlotUpdateProto` / `SnapshotProto` / `JoinRequestProto` / `JoinReplyProto` / `SetRequestProto` / `SetReplyProto` を定義し、`build.rs` の `buffa_build` でコンパイルして `unison::sync::proto` に公開する。
  - `spawn_feed` / `pump` および `ConnectedSlave` ↔ `SyncServer` 間の `Join` / `Set` は `ProtoCodec`（`buffa` バイナリ）で直接通信し、`pump` は `SlotUpdateProtoView::decode_view` で zero-copy デコードする。
  - 一方、`UnisonChannel<JsonCodec>` や `unison-mcp` 等の JSON クライアントから `Join` / `Set` が呼ばれた場合も、先頭バイト判定（JSON の `'{'` vs protobuf tag）により JSON でもそのまま透過的に応答できる両刀設計になっている。

### 2.2 購読者ごとの queue を持たない「新しいものだけ」の畳み込み

- `Slots` は `entries: BTreeMap<String, Entry>` と、変更順の索引 `order: BTreeMap<u64, String>`（この `Slots` 内の通し番号 `seq` → 要素名）を持つ。
- 要素が更新されると、古い `seq` の索引を消して新しい `seq` に差し替えるため、`order` の要素数は常に **要素（slot）の種類数以下** に保たれる。
- `Shared` は `tokio::sync::watch::Sender<Version>` で最新の `seq` と `closed` だけを知らせ、各 `Subscription` は自分の `cursor: u64` を使って `slots.changes_since(cursor)` を引く。

### 2.3 道を 2 本に分ける（client 起点 control + server 起点 feed）

- client 起点の `UnisonChannel` は event 受信に 256 件の固定長 queue（`try_deliver`）を使い、満杯時に黙って捨てる。要素単位の `Update` が 1 件でも捨てられると、その要素が次に更新されるまで写しが正本と食い違う。
- そのため、`Join` と `Set`（request/response）は client 起点の control チャネル（既定 `unison.sync`）で送り、変動の配信（`Update` event）は server 起点の raw `UnisonStream`（既定 `unison.sync.feed`、`ctx.open_server_stream` + `client.register_server_channel`）で直接読み書きする。

### 2.4 同時 in-flight `Set` の抑制（`MAX_SETS_IN_FLIGHT = 128`）

- Server 側の `UnisonChannel` は、届いた **request** も event と同じ 256 件の待ち行列（`try_deliver`）に通す。
- これを防ぐため、Client 側の `QuicUpstream` は `Semaphore::new(128)`（`MAX_SETS_IN_FLIGHT`）で 1 チャネルあたりの同時 in-flight `Set` 数を 128 以下に抑え、超えた分は手元で順番待ちさせる（3,000 並列 `Set` でも取りこぼしゼロで完走する）。

### 2.5 上流から外れたら返事待ちの `Set` を即座に終わらせる（専用 `closed` watch）

- `UnisonChannel::close()` は受信ループと stream を閉じるが、すでに送信して返事待ちの `request()`（`pending` map）を即座には解かず、30 秒の timeout まで待たせる。
- 上流が止まった・切れたときや `Relay::stop()` したときに返事待ちの `Slave::set*` タスクが残らないよう、`Slave` の `Link::set_bytes` は上流への `upstream.set_bytes` と `self.shared.closed`（閉じたことだけを知らせる専用 `watch::Receiver<bool>`）を `tokio::select!` する。
- 変動の知らせ（`version`）とは別の `watch::Sender<bool>` に分離しているため、変動のたびに返事待ちタスクが起床することはない（`pending_set_is_not_woken_by_updates` で検証）。

### 2.6 `Relay` は上流の返事を待つ間に lock を握らない

- `Relay::set_bytes` は `Mutex` を握って `Source::upstream(&held.slave)`（`Arc<dyn Upstream>`）を取り出した直後に lock を手放し、その後に `upstream.set_bytes(slot, payload).await` を待つ。
- これにより、ある下流の `Set` が上流の返事を待っていても、他の下流の `Join` や別の `Set` は一切ブロックされない。
