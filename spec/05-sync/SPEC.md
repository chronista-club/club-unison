# spec/05: Unison Protocol - State Sync (`unison::sync`)

**バージョン**: 0.1.0
**最終更新**: 2026-11-23
**ステータス**: Stable

---

## 1. 概要

`unison::sync` は、正本（**Origin**）と手元の写し（**Slave**）の間で、名前の付いた要素（**slot**）の集まりを裏で自動同期する仕組みである。

- 手元の利用者はネットワークを意識せず、`Slave::get` / `Slave::snapshot` で現在の値を読み、`Slave::changed` で変動を待ち、`Slave::set` で Origin に要素の置き換えを頼む。
- `Slave` 自身も `Origin` と同じ形の状態（要素ごとの `(rev, value)`）を持つため、そのまま下流の `Slave` への配り手（**中継 / Relay**）になれる。何段重ねても合流の決まりは変わらない。

---

## 2. 設計思想（What & Why）

| 項目 | 決定 | 理由 |
|---|---|---|
| **正本と写し** | 正本（`Origin`）は 1 つだけ。要素の値を確定し `rev` を 1 つ進めるのは `Origin` のみ | CRDT や multi-master の複雑さを持ち込まず、競合は「`Origin` が後から確定したほうが勝つ」の一本で解決する |
| **手元の見方** | 通信メッセージを 1 件ずつ処理するのではなく、手元の `Slave` の状態と変動を見る | 描画やアプリの消費者がネットワークや再送を意識しなくて済む |
| **合流の決まり** | `Slave` は **同じ代（`life`）で、手元より新しい `rev` だけを受け入れる** | `rev` は要素ごとに独立しているため、要素ごとに別々の道を通っても、応答と配信が入れ替わって届いても、中継を何段挟んでも矛盾なく合流する |
| **受け取りが遅い購読者** | 同じ要素の途中の変動は飛ばし、最新の値へ畳む（1 人あてに溜まるのは要素の数まで） | 遅い購読者がいても待ち行列が溢れず、最新の状態へ最短で追いつける |
| **変動駆動** | 時計（tick）は持たず、要素が置き換えられたときだけ変動を知らせる | 変わっていない状態を送り続けない |

---

## 3. 状態と操作

### 3.1 状態の形

| 概念 | 内容 |
|---|---|
| **代（`life: u64`）** | 何代めの `Origin` か。`Origin` が立ち上がり直すたびに大きくなる番号 |
| **要素（`slot: String`）** | 空でない任意の名前を持つ要素。要素ごとに独立した世代番号 `rev: u64`（最初の確定が `1`）と値 `value: serde_json::Value` を持つ |
| **`SlotState`** | 要素 1 つの状態 `{ rev, value }` |
| **`SlotUpdate`** | 要素 1 つの変動 `{ life, slot, rev, value }`。`Origin` が振った `rev` をそのまま運ぶ |
| **`Snapshot`** | ある時点の状態丸ごと `{ life, slots: BTreeMap<String, SlotState> }` |

### 3.2 操作

| 操作 | 向き | 内容 |
|---|---|---|
| **入る（`Join` / `subscribe` / `follow`）** | 外 → `Origin`（または `Relay`） | その時点の `Snapshot` を手元の `Slave` に写し、以後の変動を受け始める |
| **出ていく（drop / `leave`）** | `Slave` → 上流 | 以後の変動を受け取らなくなる。その `Slave` を通して入っていた下流も外れる。上流は動き続ける |
| **置き換える（`Origin::set`）** | 持ち主 → `Origin` | 要素の値を置き換えて確定する。その要素の `rev` が 1 つ進み、入っている全員へ変動を知らせる |
| **置き換えを頼む（`Slave::set`）** | `Slave` → `Origin` | `Origin` が確定するまで `Origin` も手元も変わらない。確定したら全員に `feed` で届き、頼んだ人には応答でも返る（応答の道でも即座に合流 = read-your-writes） |
| **止まる（`Origin::close` / `Relay::stop`）** | `Origin` / `Relay` | 配り手が止まる。入っていた下流は全員外れ、確定していない返事待ちの頼みも `SyncError::Closed` で即座に終わる |

---

## 4. 変動の道のりと形式検証（Quint）

知らせをどの道で運び、受け取る側がどんな決まりを持つかで成り立つ性質が変わる。Quint 形式仕様モデル（7 通りの道のり）で比較検証した結果、実装は **`streamLatestMerge`（順番どおり・新しいものだけ・応答の道あり・受け取る側で古い `rev` と別の `life` を捨てる）** を採用している。

### 4.1 7 通りの道のりの比較結果

○ = 成り立つ ／ × = 破れる（反例が見つかる）

| 性質 | モデル上の名前 | 順不同・上書き (`dgramNaive`) | 順不同・古い rev を捨てる (`dgramDiscard`) | 順不同・前の代も捨てる (`dgramDiscardLife`) | 順番どおり・全部 (`streamAll`) | 順番どおり・新しいものだけ (`streamLatest`) | ＋応答の道 (`streamLatestEcho`) | **＋応答の道・捨てる (`streamLatestMerge`)** |
|---|---|---|---|---|---|---|---|---|
| 基本の 7 不変条件（手元が Origin より先に進まない等） | `allInvariants` | × | × | ○ | ○ | ○ | ○ | **○** |
| 手元の `rev` は戻らない | `seenNeverGoesBack` | × | ○ | ○ | ○ | ○ | × | **○** |
| 途中の知らせがもう無いなら、全員の手元は Origin と同じ | `quietMeansCurrent` | × | × | × | ○ | ○ | × | **○** |
| 1 人・1 要素あてに溜まる知らせは頭打ち | `backlogAtMostTwo` | × | × | × | × | ○ | ○ | **○** |
| 入っている人は常に今の状態を持っている（遅延ゼロ） | `alwaysCurrent` | × | × | × | × | × | × | **×**（非同期通信では約束しない） |

### 4.2 重要な反例（なぜ `streamLatestMerge` が必要か）

- **応答の道による追い越し（反例 5）**:
  `streamLatest`（1 本の順番どおりの道）だけでは受け取り側の `rev` 検査は不要に見えるが、頼んだ人への **応答の道（`SetReply`）** を足すと（`streamLatestEcho`）、
  「`Slave` が頼む（`rev 1` 確定）→ 直後に別更新（`rev 2` 確定）→ `feed` で `rev 1`, `rev 2` が先に届く → 遅れて `SetReply(rev 1)` が届く」
  という順で手元が `rev 1` に巻き戻る。
- **解決**:
  `feed` と `SetReply` の両方を、同じ合流規則 **`Slots::merge`（同じ `life` で、手元より大きい `rev` だけを受け入れる）** に通す（`streamLatestMerge`）。これにより、道を何本に分けても（応答、要素別 stream、多段中継）、手元は決して戻らず、通信が落ち着けば全員が `Origin` と一致する。

---

## 5. KDL スキーマ定義

標準の 2 チャネル構成は [`schemas/sync.kdl`](../../schemas/sync.kdl) を正本とする：

- **`unison.sync`（client 起点、persistent）**:
  - `Join` request → `JoinReply { life, slots, error }`
  - `Set { slot, value }` request → `SetReply { life, slot, rev, value, error }`
- **`unison.sync.feed`（server 起点、persistent）**:
  - `Update { life, slot, rev, value }` event

> **なぜ `feed` を server 起点 stream に分けるか**:
> client 起点 `UnisonChannel` の event 受信待ち行列（256 件上限）は満杯時に古い/新しい event を暗黙ドロップする（[spec/02 §5.5](../02-unified-channel/SPEC.md)）。要素別の `Update` が 1 つでも落ちると、その要素が次に更新されるまで古い値のまま残る（反例 3）。server 起点 stream（`open_server_stream` + `register_server_channel` 直読）は中継 queue を挟まず、受け取りが遅ければ QUIC の流量制御で送信側を待たせ、待たされている間の更新は `Subscription` が最新値へ畳む。

---

## 6. 関連ドキュメント

- [design/sync.md](../../design/sync.md) — `unison::sync` の内部構造と設計判断（D14 / D21 / D25 等）
- [schemas/sync.kdl](../../schemas/sync.kdl) — KDL スキーマ定義
- [design/server-initiated-stream.md](../../design/server-initiated-stream.md) — Server 起点 reliable stream
