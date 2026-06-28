# Server-Initiated Stream — `ServerToClient` 方向を起こす（reliable server→client）

**status**: 設計確定 2026-06-28 / 1.5.0 実装中
**動機元**: chronista-hub federation relay（ADR-020 §S4）— hub が world A の bytes を world B の既存 connection へ **reliable** に forward する floor が要る。
**原則**: Occam — 新 transport 概念を増やさず、**既に宣言されているが眠っている `ChannelDirection::ServerToClient` を honor する**だけ。

---

## 1. 何が欠けているか

club-unison の現状（1.4.0）:

- **`ChannelDirection::ServerToClient` は型にも KDL（`from="server"`）にも既出**だが、`build_identity()` が常に `Bidirectional` をハードコードし runtime で無視している。
- server→client の唯一の経路 = **client が開いた persistent channel 上で `UnisonChannel::send_event`**。これは設計どおり（`dispatch.rs`「server→client 通信は client が開いた channel 上で行う」）だが、**意図的に best-effort**: recv demux が event を `try_send`・buffer 256・full で **drop + warn**（Response path を HoL させないため、`channel.rs` の `try_deliver`）。
- 帰結: **reliable な server→client 配送が無い**。Request/Response（oneshot）だけが reliable で、それは client→server 起点に限る。

relay のように「server 起点で取りこぼせない stream」を運ぶ用途には、best-effort event では不十分（backpressure 下で drop → 上位の再送 churn）。

## 2. 既にある材料

- **`UnisonConn::open_bi() -> BiStream`**（`conn.rs`）は **server からも stream を開ける**。現に identity 送信が `connection.open_bi()` で server-initiated stream を1本開いている（`dispatch.rs`）。
  → **「server が stream を開く動詞」は新設不要**。
- client 側 `client_accept_bi_loop`（`dispatch.rs`）は server-initiated bi stream を `accept_bi()` で受けているが、**先頭 frame の method が `__identity` 以外は drop** している。
  → routing を一般化すれば、宣言済みの server-push channel に配れる。

## 3. 最小 primitive（対称2編集）

新概念ゼロ。`ServerToClient` を本物にする2点だけ。

### ① server 側 — handler が server-initiated stream を開けるようにする

`ConnectionContext` に connection を持たせ（ctx を作る `dispatch.rs::handle_connection` で `Arc<dyn UnisonConn>` は手元にある）、handler から reliable stream を開く verb を生やす:

```rust
// ConnectionContext
conn: Arc<dyn UnisonConn>,   // 追加（既に handle_connection で利用可能）

/// server 起点で reliable な双方向 stream を開き、宣言 channel として frame する。
/// 中身は既存 conn.open_bi() + 先頭に channel 宣言 frame（__identity と同じ要領）。
pub async fn open_server_stream(&self, channel: &str) -> Result<UnisonChannel, NetworkError>;
```

- 返る `UnisonChannel` は **dedicated QUIC stream**（flow-control 内蔵 = reliable・取りこぼさない）。
- `channel` 名は client 側の `from="server"` handler 解決キー。

### ② client 側 — server-initiated stream を宣言 channel handler へ routing

client が **server-push channel handler を登録**できるようにし、`client_accept_bi_loop` が先頭 frame の channel/method で振り分ける（`__identity` は従来どおり専用 oneshot、それ以外は登録 handler へ、未登録なら従来どおり drop+warn）:

```rust
// ProtocolClient
pub async fn register_server_channel<F>(&self, channel: &str, handler: F)
where F: Fn(UnisonChannel) -> BoxFuture<'static, Result<()>> + Send + Sync + 'static;
```

→ `from="server"` channel を declared に受理。`__identity` の特別扱いはこの一般機構の一例に縮約される（既存挙動は不変）。

## 4. 意図的に**足さない**もの（剃刀）

- **`get_connection_by_principal` / connection lookup table は substrate に置かない**。「どの principal/id が誰か」は **application の関心**。利用側（hub）が自分で `key → Arc<UnisonConn>`（or ctx）map を持つ。substrate は「この connection に server stream を開く」だけを提供する。
- **relay 専用 API も置かない**。relay は利用側の composition。substrate は汎用の server-initiated stream のみ。
- 新しい transport 動詞（`open_bi` 以外）を増やさない。

## 5. framing / 後方互換

- server-initiated stream の先頭 frame で channel/method を宣言（`__identity` と同形）。client はそれで handler を解決。
- **additive**: 既存の client-opened channel・`send_event`・request/response は不変。`from="server"` handler を登録しない client は server-push stream を従来どおり drop（無回帰）。
- `build_identity()` は `from="server"` を honor して `ServerToClient` を返すよう更新（schema と runtime の齟齬解消、自己記述 discovery が正しくなる）。

## 6. reliability の根拠

dedicated QUIC stream は per-stream flow-control を持つ → 送り手は drop せず **backpressure で slow down**。best-effort event（`try_send` drop）と違い relay payload を取りこぼさない。これが §S4 で Arch B でなく A を選んだ核心。

## 7. version

**1.5.0**（minor・additive）。下流（chronista-hub relay / VP dialer・target inbound）はこの上に composition で乗る。

## 8. 実装 checklist（1.5.0、この repo で実施）

> handoff 元: chronista-hub federation session（2026-06-28）。本 doc が SSOT。下流 = chronista-hub relay（§S4）/ VP dialer は 1.5.0 release 後に乗る。

1. **`network/context.rs`** — `ConnectionContext` に `conn: Arc<RwLock<Option<Arc<dyn UnisonConn>>>>`（init None）+ `set_conn(conn)` + `open_server_stream(channel) -> Result<UnisonChannel>`。`open_server_stream` = `conn.open_bi()` →（宣言 frame を `write_typed_frame` で1本書く、`__identity` 送信と同形 = `dispatch.rs::handle_connection` L129-140 参照）→ `UnisonStream::from_streams` → `UnisonChannel::new`。conn が None なら error（client 誤用）。`#[derive(Debug)]` は `UnisonConn` 非 Debug なので**手書き Debug** へ（conn field を skip）。
2. **`network/dispatch.rs`** — `handle_connection` 冒頭で `ctx.set_conn(Arc::clone(&connection)).await`（**server 側のみ・1行**。call-site ripple ゼロ = ctx/conn は既にここで同居）。`client_accept_bi_loop` を一般化: 先頭 frame の method が `__identity` → 従来の oneshot、それ以外 → client 側 server-channel registry を引いて handler へ（`UnisonChannel` 化して渡す）、**未登録なら従来どおり drop+warn**（無回帰）。
3. **`network/client.rs`** — `ProtocolClient` に server-channel handler registry（`Arc<RwLock<HashMap<String, Handler>>>`）+ `register_server_channel(channel, handler)`。`client_accept_bi_loop` 起動箇所（`quic.rs` L376）へ registry を渡す。
4. **`network/identity.rs` / `server.rs`** — `build_identity()` が `from="server"` を honor して `ChannelDirection::ServerToClient` を返す（schema↔runtime 齟齬解消、二次的）。
5. **tests** — server が `open_server_stream("x")` → client の `register_server_channel("x", ...)` handler が **reliable に受信**（取りこぼし無し round-trip）。後方互換: handler 未登録 client は従来 drop。
6. **version** — workspace `Cargo.toml` 1.4.0 → **1.5.0**（minor・additive）。CHANGELOG。

**剃刀の不変条件**（実装中に侵さないこと）: `get_connection_by_principal` 等の lookup table を substrate に置かない（利用側=hub が `wld_id→conn` map を持つ）。relay 専用 API も置かない（relay は利用側 composition）。新 transport 動詞を増やさない（`open_bi` 既存）。

## References

- chronista-hub ADR-020 §S4（relay = universal floor、本 primitive の利用側）
- `design/connection-auth.md`（connection-level の principal — relay の wld_id↔conn map は利用側がこの principal を keyに持つ）
- `network/dispatch.rs`（`client_accept_bi_loop` / `handle_connection`）/ `network/conn.rs`（`UnisonConn::open_bi`）/ `network/context.rs`（`ConnectionContext`）/ `network/channel.rs`（`send_event` best-effort demux）
