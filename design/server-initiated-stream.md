# Server-Initiated Stream — `ServerToClient` 方向を起こす（reliable server→client）

**status**: 設計確定 2026-06-28 / 対称化路線へ再構成 2026-06-28 / 1.5.0 実装中
**動機元**: chronista-hub federation relay（ADR-020 §S4）— hub が world A の bytes を world B の既存 connection へ **reliable** に forward する floor が要る。
**原則**: Occam — 新 transport 概念を増やさず、**既に宣言されているが眠っている `ChannelDirection::ServerToClient` を honor する**だけ。さらに本改訂では、reliable を「新しい配送 mode」ではなく **server / client の受信 handler を対称化する**ことで構造的に得る。

---

## 1. message の 2 軸モデル — なぜ `ServerToClient` が要るか

inter-agent / relay の通信を 2 つの**直交する軸**で捉えると、欠けているものが 1 つに絞れる。

- **interaction 軸**: 一方向（fire-and-forget）/ 往復（request_id で相関する req↔resp）
  - 往復は transport の別 mode ではなく「correlation_id で結んだ 2 本の一方向 message」= 規約。`UnisonChannel` の pending oneshot map がこれを実装している。
- **QoS 軸**: best-effort（落ちてよい）/ reliable（落ちてはいけない）

|              | best-effort（落ちてOK）          | reliable（落ちてはNG）                       |
| ------------ | -------------------------------- | -------------------------------------------- |
| **一方向**   | telemetry / position → datagram  | **relay msg ← ここだけ実装が無い**            |
| **往復**     | （ほぼ無意味）                   | req↔resp（応答脚は本質 reliable）→ 実装済み   |

→ 4 セル中 3 つは既にある。**欠けているのは「一方向 × reliable」の 1 セルだけ**で、それを **server 起点で**開けるようにするのが本 primitive。これは型・KDL に既出の `ChannelDirection::ServerToClient`（`from="server"`）そのものを指す。

### lane モデル（QoS クラス = stream を分ける）

QUIC の idiom「用途ごとに stream を分ける（= 独立 HoL 境界）」に従い、QoS クラスごとにレーンを割る:

| lane                          | transport                  | 信頼性 / 順序            | 用途                       | 現状     |
| ----------------------------- | -------------------------- | ----------------------- | -------------------------- | -------- |
| real-time state               | datagram                   | best-effort / 無順序     | position / rotation        | 稼働中   |
| continuous media              | raw frame・専用 stream     | reliable / 同順          | audio                      | 予約     |
| **app protocol**              | structured 0x00・専用 stream | reliable / 同順          | req↔resp + 意味のある event | 稼働中（client 開設のみ）／**本 primitive で server 起点を追加** |

**帰結**: best-effort(drop) は datagram に隔離される。**stream lane に best-effort が残る理由は無く、stream lane は全部 reliable であるべき**。`send_event` の `try_send`-drop は「telemetry を structured lane に混ぜていた時代の遺物」と位置づける（§4 / §7）。

---

## 2. 何が欠けているか（現状 1.4.0）

- **`ChannelDirection::ServerToClient` は型にも KDL（`from="server"`）にも既出**だが、`build_identity()`（`network/server.rs:208`）が常に `Bidirectional` をハードコードし runtime で無視している。
- server→client の唯一の経路 = **client が開いた persistent channel 上で `UnisonChannel::send_event`**。これは `dispatch.rs`「server→client 通信は client が開いた channel 上で行う」どおりだが、**意図的に best-effort**: client 側 `UnisonChannel` の recv ループが event を `try_send`・buffer 256・full で **drop + warn**（`channel.rs:74-85` の `try_deliver`）。
- 帰結: **reliable な server-initiated 配送が無い**。

relay のように「server 起点で取りこぼせない stream」を運ぶ用途には best-effort event では不十分（backpressure 下で drop → 上位の再送 churn）。

---

## 3. 既にある材料

- **`UnisonConn::open_bi() -> BiStream`**（`conn.rs`）は **server からも stream を開ける**。現に identity 送信が `connection.open_bi()` で server-initiated stream を 1 本開いている（`dispatch.rs:133`）。
  → **「server が stream を開く動詞」は新設不要**。
- **server 側 channel handler は既に raw `UnisonStream` を直接 read している**（`dispatch.rs:222-229` で `UnisonStream::from_streams` を作って `handler(ctx, stream)`、discovery handler も同様）。
  → これが**対称化の土台**。reliable な受信の「正解の形」は既に server 側に実在する（§4）。
- client 側 `client_accept_bi_loop`（`dispatch.rs:27`）は server-initiated bi stream を `accept_bi()` で受けているが、**先頭 frame の method が `__identity` 以外は drop** している。
  → routing を一般化すれば、宣言済みの server-push channel に配れる。

---

## 4. 設計：対称化（client handler も raw `UnisonStream` を受け取る）

新概念ゼロ。`ServerToClient` を本物にする対称 2 編集だけ。

### ① server 側 — handler が server-initiated stream を開けるようにする

`ConnectionContext` に connection を持たせ（ctx を作る `dispatch.rs::handle_connection` で `Arc<dyn UnisonConn>` は手元にある）、handler から reliable stream を開く verb を生やす:

```rust
// ConnectionContext
conn: Arc<RwLock<Option<Arc<dyn UnisonConn>>>>,   // 追加（init None、server 側のみ set される）

/// server 起点で reliable な双方向 stream を開き、宣言 channel として frame する。
/// 中身は既存 conn.open_bi() + 先頭に channel 宣言 frame（__identity と同要領）。
/// finish() はしない（persistent stream）。
pub async fn open_server_stream(&self, channel: &str) -> Result<UnisonStream, NetworkError>;
```

- 返るのは **`UnisonStream`（raw）** であって `UnisonChannel` ではない。これが対称化の肝。
- `channel` 名は client 側の `from="server"` handler 解決キー。

### ② client 側 — server-initiated stream を宣言 channel handler へ routing

client が **server-push channel handler を登録**できるようにし、handler には **`UnisonStream`（raw）を渡す**（= server handler と同じ面）:

```rust
// ProtocolClient
pub async fn register_server_channel<F>(&self, channel: &str, handler: F)
where F: Fn(UnisonStream) -> BoxFuture<'static, Result<()>> + Send + Sync + 'static;
```

`client_accept_bi_loop` が先頭 frame の method で振り分ける（`__identity` は従来どおり専用 oneshot、それ以外は registry を引いて handler へ `UnisonStream` を渡す、未登録なら従来どおり drop+warn）。

### なぜ `UnisonStream`（直読）であって `UnisonChannel::new_reliable`（mpsc + `send().await`）でないか

- **reliable の正解は actor / 直読**。handler 自身のタスクが `stream.recv_typed_frame().await` を順に読む。**recv_task も中継 mpsc も無いので drop が原理的に起きず、deadlock も起きない**（1 task が Response も event も順に捌く）。server 側が既にこの形。
- 対して `UnisonChannel`（client 側）の `try_send`-drop は、**req↔resp と event を 1 本に混ぜた共有 channel**で「event 配送を block すると後ろの Response が詰まる」HoL/deadlock を避けるためのブレーカー。**ここを global に `send().await` へ flip すると deadlock が戻る**（drain loop の中で `request().await` して park → event が溜まる → recv_task が block → その request の Response が読めず詰む）。
- つまり drop は **client 側 mpsc bridge の遺物**であり、寄せるべき先は「mpsc を blocking 化」ではなく「**mpsc を外して server と同じ直読に揃える**」。本 primitive はその対称化を server-initiated stream について行う。
- 既存 client `open_channel` → `UnisonChannel`(drop) は**触らない**。client 全体を直読（actor）へ寄せる完全 unification は north star（§7）。

---

## 5. reliability の根拠

`★ 完全性は queue でなく end-to-end backpressure（QUIC flow control）が生む。`

- handler が `recv_typed_frame()` を**読まない間**、QUIC の受信 window が埋まる → QUIC が送信側へ flow control → 送信側（hub）の write が block。backpressure が network 越しに送信元まで伝播する。
- **1 byte も落とさず**、メモリは QUIC stream window で**有界**。
- best-effort event（`try_send` drop）との違いはここ: dedicated QUIC stream + 直読は「詰まったら drop」でなく「詰まったら遡って throttle」。これが §S4 で Arch B でなく A を選んだ核心。
- queue 単体（bounded→drop / unbounded→OOM）では完全性は出ない。QUIC を底に敷いて初めて「有界 × 無損失」が成立する。

---

## 6. framing / 後方互換

- server-initiated stream の先頭 frame で channel/method を宣言（`__identity` と同形 = `FRAME_TYPE_PROTOCOL` の `ProtocolMessage`、method = channel 名）。client はそれで handler を解決し、**宣言 frame は routing で消費**、handler は後続 payload を読む（server 側の channel open と同じ要領）。
- client 側 routing: 先頭 frame の method が `__identity` → 従来の oneshot、それ以外 → registry 引き、未登録なら従来どおり **drop + warn**（無回帰）。
- **additive**: 既存の client-opened channel・`send_event`・request/response は不変。`from="server"` handler を登録しない client は server-push stream を従来どおり drop（無回帰）。
- `build_identity()` は `from="server"` を honor して `ServerToClient` を返すよう更新（schema↔runtime 齟齬解消）。**二次的**で、server 側に server-push channel の direction 真実が無い現状（`register_channel` に direction 引数が無い）では read 元の整備が前提（§8）。

---

## 7. 意図的に**足さない**もの（剃刀）

- **`get_connection_by_principal` / connection lookup table は substrate に置かない**。「どの principal/id が誰か」は application の関心。利用側（hub）が `wld_id → ctx`（or `Arc<UnisonConn>`）map を持ち、`ctx.open_server_stream()` を呼ぶ。
- **relay 専用 API も置かない**。relay は利用側の composition。substrate は汎用の server-initiated stream のみ。
- **新しい transport 動詞（`open_bi` 以外）を増やさない。**
- **aggregate（1 request → N response 等の多重相関）は作らない**。pending を `oneshot | mpsc` に一般化する必要が出るが、consumer が現れるまで保留。本 primitive のスコープ外。
- **既存 client `UnisonChannel` の global reliable 化（actor/直読への全面 unification）はやらない**。relay でパターンが実証できた後の north star。1.5.0 は server-initiated handler の対称化に閉じる。
- **WebTransport（browser）client の server-initiated 受理は意図的に defer**（§8 の制約は「考慮漏れ」でなく剃刀による保留）。理由: federation の relay target は **常に native World**（home-World daemon = federation peer）で、browser は peer でなく **World の client** → relay の server 発信 push が browser に直接届く経路は構造上存在しない。browser↔World hop も persistent channel 1 本で足りる（reliable = request/response 脚、best-effort = event）→「browser が World 発信 reliable stream を unsolicited に受ける」consumer が不在。必要が出た時の解消は **additive**（TS client に `register_server_channel` + 先頭 frame routing、wire は同じ宣言 frame で互換）= 早く作る利得ゼロ・待っても debt ゼロ。

---

## 8. 既知の制約 / 実装時の落とし穴

- **client 受信経路は raw QUIC 専用**。`client_accept_bi_loop`（`dispatch.rs:27`、起動 `quic.rs:376`）は `quinn::Connection` 直で、`Arc<dyn UnisonConn>` を貫通していない。→ **WebTransport（ブラウザ）client は server-initiated channel を受けられない**（native/quinn client は OK）。`UnisonStream::from_streams` に渡す `Arc<dyn UnisonConn>` は `Arc::new(connection.clone())` で作れる（既に `client.rs:212` で使う pattern）。透過の完全化は別 issue。
- **server-initiated stream に nack（配送失敗シグナル）が無い**。client-opened channel は open_ack/nack を持つ（`dispatch.rs:206` / `client.rs:190`）が、server-initiated は宣言 frame を書いて流すだけ。client が handler 未登録なら drop + warn で、**server は失敗を知れない**（silent blackhole）。relay は「downstream が handler を登録済み」を前提に運用する。明示 ack が必要になったら別途。
- **`client_accept_bi_loop` は現状 1 frame 読んで task 終了 + `_send_stream` を破棄**（`dispatch.rs:33`）。identity は 1 frame で正だが、persistent server channel 化には **send_stream を保持**して `UnisonStream::from_streams` に渡し、handler に**継続 read**させる必要。
- **`ConnectionContext` の `#[derive(Debug)]`**: `UnisonConn` は Debug 非実装（`conn.rs:71`）なので、conn field を持たせると derive が壊れる → **手書き Debug**（conn を skip）へ。
- **`conn` は `Option` + `set_conn`**: ctx 生成は 3 つの server ingress に散る（`webtransport.rs:314` / `quic.rs:638,672`）が、全て `handle_connection` に収束するので、そこで `set_conn` 1 行で足りる（call-site ripple ゼロ）。client ctx（`client.rs:98,117`）は conn=None のままで `open_server_stream` が error → 正しい誤用検知。

---

## 9. version

**1.5.0**（minor・additive）。下流（chronista-hub relay / VP dialer・target inbound）はこの上に composition で乗る。

---

## 10. 実装 checklist（1.5.0、この repo で実施）

> handoff 元: chronista-hub federation session（2026-06-28）。本 doc が SSOT。下流 = chronista-hub relay（§S4）/ VP dialer は 1.5.0 release 後に乗る。

1. **`network/context.rs`** — `ConnectionContext` に `conn: Arc<RwLock<Option<Arc<dyn UnisonConn>>>>`（init None）+ `set_conn(conn)` + `open_server_stream(channel) -> Result<UnisonStream>`。`open_server_stream` = `conn.open_bi()` →（宣言 frame を `write_typed_frame` で 1 本書く、`__identity` 送信と同形 = `dispatch.rs::handle_connection` 参照、**`finish()` しない**）→ `UnisonStream::from_streams`。conn が None なら error（client 誤用）。`#[derive(Debug)]` → conn を skip した**手書き Debug**へ。
2. **`network/dispatch.rs`** — `handle_connection` 冒頭で `ctx.set_conn(Arc::clone(&connection)).await`（**server 側のみ・1 行**）。`client_accept_bi_loop` を一般化: 先頭 frame の method が `__identity` → 従来 oneshot、それ以外 → client 側 server-channel registry を引いて handler へ **`UnisonStream` を渡す**（**send_stream を保持**し、`Arc::new(connection.clone())` で `Arc<dyn UnisonConn>` を作って `from_streams`）、**未登録なら drop+warn**（無回帰）。
3. **`network/client.rs`** — `ProtocolClient` に server-channel handler registry（`Arc<RwLock<HashMap<String, Handler>>>`、`Handler = Fn(UnisonStream) -> BoxFuture<...>`）+ `register_server_channel(channel, handler)`。`client_accept_bi_loop` 起動箇所（`quic.rs:376`）へ registry を渡す。
4. **`network/identity.rs` / `server.rs`** — `build_identity()` が `from="server"` を honor して `ChannelDirection::ServerToClient` を返す（**二次的**、direction の read 元整備が前提）。
5. **tests** —
   - server が `open_server_stream("x")` → client の `register_server_channel("x", ...)` handler が **reliable に受信**（取りこぼし無し round-trip）。
   - **backpressure**: handler が読まないと送信側 write が throttle され、**drop が起きない**（unbounded 成長もしない）。
   - 後方互換: handler 未登録 client は従来 drop+warn。
6. **version** — workspace `Cargo.toml` 1.4.0 → **1.5.0**（minor・additive）。CHANGELOG。

**剃刀の不変条件**（実装中に侵さないこと）: `get_connection_by_principal` 等の lookup table を substrate に置かない（利用側=hub が `wld_id→ctx` map を持つ）。relay 専用 API も置かない。新 transport 動詞を増やさない（`open_bi` 既存）。aggregate / 既存 `UnisonChannel` の global reliable 化はスコープ外。

---

## References

- chronista-hub ADR-020 §S4（relay = universal floor、本 primitive の利用側）
- `design/connection-auth.md`（connection-level の principal — relay の `wld_id↔conn` map は利用側がこの principal を key に持つ）
- `network/dispatch.rs`（`client_accept_bi_loop` / `handle_connection`）/ `network/conn.rs`（`UnisonConn::open_bi`）/ `network/context.rs`（`ConnectionContext`）/ `network/stream.rs`（`UnisonStream` = 直読の reliable 面）/ `network/channel.rs`（`UnisonChannel` の `try_send` best-effort demux = 触らない既存）
