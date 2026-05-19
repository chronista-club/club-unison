# 変更履歴

このプロジェクトの主要な変更はこのファイルに記録されます。

フォーマットは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に基づいており、
このプロジェクトは [セマンティックバージョニング](https://semver.org/lang/ja/) に準拠しています。

## [Unreleased]

## [1.0.0-rc.2] - 2026-05-19 — polyglot client 拡充 + CLI request/response 被覆

> rc.1 以降の追補。Ruby client gem を新設し、`unison` CLI に request 送信コマンドを追加して channel の request/event 両半分を CLI で被覆した。

### 追加 — Ruby client gem (`unison-client`)

- `clients/ruby/` — Rust `club-unison` crate を Magnus native 拡張で wrap する言語バインディング（protocol 再実装ではない）
- `Unison::Client`（接続ライフサイクル + `open_channel`）/ `Unison::Channel`（`request` / `send_event` / `recv` / `close`）/ `Unison::Error` (`< StandardError`)
- channel payload は native Ruby 値 ⇄ `serde_json::Value` を `serde_magnus` で双方向変換
- ブロッキング呼び出しは `rb_thread_call_without_gvl` で GVL を解放

### 追加 — `unison call` サブコマンド

- `unison call <url> -c <channel> -m <method> [-p <json>] [--timeout <ms>]` — channel に request を 1 本送り response を pretty JSON 出力
- `mock`（サーバ応答）と対になり、CLI だけで request/response ループが閉じる

### 変更 — `unison ping`

- 接続時に server identity（name / version / namespace）を表示

### 変更 — 接続 URL scheme

- canonical scheme を `quic://` に統一（`connect` は `quic://` / `https://` / `http://` / bare を受理）

## [1.0.0-rc.1] - 2026-05-17 — v1.0 polyglot client base (release candidate)

> v1.0 sprint「polyglot client base」 の release candidate。 TypeScript client SDK を新設し、 **browser から Rust server へ実 WebTransport で接続**できる状態に到達。 dogfood 開始点 (= Vantage Point ほか chronista-club ecosystem での実利用検証)。 GA は dogfood exit criteria (3+ caller × 実運用 × critical bug 0) 達成後。

### 追加 — TypeScript client SDK (`@chronista-club/unison-client`)

- `transport/` — WebTransport adapter (browser native WebTransport で QUIC server に接続)
- `channel/` — `UnisonChannel` (stream: request/response + event) / `DatagramChannel` (datagram broadcast) + varint demux dispatcher
- `codec/` — `JsonCodec` / `ProtoCodec` (buffa proto3 互換)
- `error/` — `ErrorCategory` (Transport / Protocol / Application / Resource)
- `wire/` — Rust `ProtocolMessage` と byte-identical な proto3 wire codec
- `UnisonClient` facade — `connect()` → `openChannel` / `openDatagramChannel`、 型安全 (codegen の `__types` carrier で生成 interface に narrowing)

### 追加 — WebTransport server endpoint + cross-language interop

- transport 抽象 `UnisonConn` trait — quinn raw QUIC と WebTransport session を同一 `handle_connection` に合流
- Rust server に `wtransport` ベースの WebTransport endpoint (browser ingress)
- identity handshake / channel `open_ack` (server-side accept signal)
- 実 WebTransport E2E (TS SDK ↔ Rust server) を CI (GitHub Actions) で検証

### 追加 — `unison` CLI dev tools

- `ping` / `sniff` / `mock` / `schema-lint` サブコマンド

### 変更 — lib 名を bare name に (`club_unison` → `unison`)

- `crates/unison-protocol/Cargo.toml` の `[lib].name`: `club_unison` → **`unison`** (chronista-club 命名規則適合)
- 全 `use club_unison::...` → **`use unison::...`** (41 ファイル)。 crates.io package 名 `club-unison` と dep 行は不変

### 変更 — codegen を channel narrative 一本化

- 旧 `service` / WebSocket codegen を Rust / TS 両 generator から除去 (CLAUDE.md「Legacy 残さない」)
- payload 型 narrowing / `connect` 命名 / `openChannel` accept signal を整備 (beta freeze blockers)

### ドキュメント

- `guides/` に quickstart / migration / typescript-sdk リファレンスを追加

### v1.x 送り (rc 期間中 or GA 後)

- club-kdl-codegen への codegen 載せ替え (IR ベース化) / proto-descriptor codegen / datagram meta codegen / Node native WebTransport / Safari・Firefox 公式対応

## [0.10.1] - 2026-05-16 — 「benchmark fresh baseline + datagram channel 計測強化」 patch

> v0.10.1 のテーマは **「v0.9.0 buffa pivot + v0.10.0 datagram channel pivot を実測で裏付け」**。 内部実装大変更後の数字を fresh baseline として記録、 datagram channel の position sync use case + max throughput 計測を追加。 純粋 additive (= wire / caller code 互換 100%)。

### 追加 — 新規 bench 3 件

#### `benches/datagram_channel.rs` — channel API 経由 burst (= v0.10.0 で導入された新 path の overhead 計測)

- 既存 `datagram.rs` (= raw connection-level、 v0.9.0 MVP) と同じ payload × burst パラメータで並列、 RESULTS.md 上で raw vs channel の overhead 比較可能
- 主な発見: JSON codec で `Vec<u8>` を encode すると **wire size が ~4x 拡大** (= 1300B input → 5200B wire)、 MTU 超過で全 drop。 caller 向け推奨「JsonCodec + datagram の effective payload limit ≈ 200-300 B」 を documentation

#### `benches/datagram_channel_sustained.rs` — 位置同期 use case の realistic shape (= 60Hz / 120Hz × 数秒 sustained stream)

- `Arc<DatagramChannel>` で send / recv 別 task の continuous streaming pattern
- 計測: 60Hz / 120Hz × 2 sec、 Transform struct (= peer id + pos + rot、 JSON wire 110-130B)
- 結果 (= Mac M-series localhost): **drop 0%** at 60Hz / 120Hz、 v0.10.0 datagram channel API は realistic single-peer position sync で fully reliable

#### `benches/datagram_channel_max_throughput.rs` — system ceiling 計測

- rate 制限なし、 2 sec で as-fast-as-possible 送信、 上限値を露呈
- 結果 (= Mac M-series localhost): **send ~530k msg/s、 recv ~445k msg/s、 drop ~2.7%**
- caller の capacity planning 数字: 「60Hz × 1 peer = 60 msg/s に対し ~7,400x headroom」

### 修正 — 既存 bench の OS-level 衝突回避

#### `benches/datagram.rs` — 固定 port (`26000+counter`) → OS-assigned port (`port 0` + `local_addr` read) に移行

- macOS 環境で AddrInUse / EAGAIN panic を回避、 安定計測可能に
- semantic 変更: per-iter cold-start → **steady-state (= 1 connection 共有 + iter_custom)** に切替、 macOS の ephemeral port / fd 枯渇問題を回避
- 数値も併せて変わるため、 v0.9.0 baseline (= cold-start semantic) との直接比較不可、 RESULTS.md で fresh baseline 宣言

### 変更 — RESULTS.md fresh baseline 化

`benches/RESULTS.md` を **v0.10.1 を新規 baseline とする** 形に rewrite。 v0.9.0 baseline (= 2026-05-15、 cold-start semantic) は git history に残し、 file 上は履歴から除外。 理由:

- v0.9.0 → v0.10.0 で buffa pivot (= rkyv → buffa、 wire format 全面切替) + datagram channel API 追加 = 内部実装大変更
- v0.10.1 で bench code 自体も rewrite (= steady-state semantic、 OS-assigned port、 shared connection)
- 過去数字との直接比較は misleading、 fresh baseline で今後の patch / minor で diff を計測する方が honest

### 計測結果 summary (= Mac M-series macOS arm64)

| Bench | Case | Result |
|---|---|---|
| `datagram` (raw) | 64B × 100 burst | 127 µs / iter |
| `datagram` (raw) | 64B × 1000 burst | 665 µs / iter |
| `datagram` (raw) | 1300B × 100 burst | 31.9 ms / iter |
| `datagram` (raw) | 1300B × 1000 burst | 507 ms / iter |
| `datagram_channel` (JSON) | 64B × 100 burst | 620 µs / iter (= raw 比 4.7x、 JSON encode 支配) |
| `datagram_channel` (JSON) | 64B × 1000 burst | ⚠️ 多数 drop + timeout 貼り付き |
| `datagram_channel` (JSON) | 1300B × any | ⚠️ JSON で MTU 超過、 全 drop |
| `datagram_channel_sustained` | 60Hz × 2sec | drop 0%、 session 2.32s |
| `datagram_channel_sustained` | 120Hz × 2sec | drop 0%、 session 2.32s |
| `datagram_channel_max_throughput` | unlimited × 2sec | **send 530k/s、 recv 445k/s、 drop 2.7%** |
| `ping_pong` (stream channel) | 16/64/256/1024 B | ~155 ms / iter (= payload non-sensitive) |

### v0.11+ への引き継ぎ

- **cloud / WAN bench**: 上記 ceiling は localhost (= 同 machine)、 同 host container / 同 AZ / cross-AZ / cross-region の realistic deployment 数字を測る docker-compose + CI integration を v0.11+ で追加
- **multi-peer broadcast bench**: `server.broadcast` を 10 / 100 / 1000 client に対して、 drop 始まる threshold 計測
- **ProtoCodec vs JsonCodec 比較**: 同 Transform で codec のみ切替、 channel API overhead が JSON 支配 (= 4.7x の 95%) であることを ProtoCodec で 1x 近くまで圧縮できる仮説の検証
- **higher rate sustained**: 240 Hz / 480 Hz position sync (= VR headset 想定)
- **`throughput.rs` / `quic_performance.rs` rewrite**: 固定 port `8080-8084` の AddrInUse 問題、 v0.10.1 では skip、 v0.11+ で OS-assigned port + steady-state semantic に統一
- **bench harness 独自化検討**: criterion の「time per iter」 だけでは sustained throughput / drop rate を表現しにくい、 custom harness or criterion 拡張
- **CI 上での bench 定期実行 + RESULTS.md auto regen**: team-b dispatch で v0.11+ で自動化

### Tests / lint

- workspace tests: 202 passed / 0 failed (= v0.10.0 と同数、 regression なし)
- integration tests (`--ignored`): 7 passed / 0 failed
- clippy clean

## [0.10.0] - 2026-05-15 — 「channel API 拡張 + 対称性向上」 release

> v0.10.0 のテーマは **「KDL channel narrative に datagram backend を統合 + ProtocolClient connection event hook で server 側との API 対称化」**。 v0.9.0 で発見された API 非対称 3 件 (= datagram server-side / client connection events / ClientIdentity) のうち 2 件を採用、 ClientIdentity は v0.11+ に deferred。 既存 v0.9.0 caller は 100% 無改修で動作 (= 純粋 additive release)。

### 追加 — Datagram channel API 統合 (KDL schema 拡張)

KDL channel に `backend="datagram"` + `channel_id=N` 属性を追加、 既存 `backend="stream"` (= v0.9.0 default) と並列に datagram channel を宣言可能に。 詳細設計は [`design/datagram-channel.md`](https://github.com/chronista-club/club-unison/blob/main/design/datagram-channel.md) と [`spec/02-unified-channel/SPEC.md`](https://github.com/chronista-club/club-unison/blob/main/spec/02-unified-channel/SPEC.md) §8.5。

#### KDL syntax 例

```kdl
channel "position" from="server" lifetime="persistent" backend="datagram" channel_id=1 {
    event "Transform" {
        field "id" type="string"
        field "pos" type="json"   // [x, y, z]
        field "rot" type="json"   // [x, y, z, w]
    }
}
```

- `backend` 属性: `"stream"` (default、 v0.9.0 互換) / `"datagram"`
- `channel_id` 属性: `backend="datagram"` 時のみ必須、 1..u64::MAX の正整数 (= proto3 field number 哲学、 author 明示割り当て)
- 1 channel = 1 backend (strict)、 mixed event は disallow (= v0.11+ で再評価)
- `backend="datagram"` channel は `request` ブロックを持てない (= unordered/unreliable で Request/Response 不適合、 `event` のみ許可)

#### Wire format (datagram payload layout)

```text
[varint channel_id] [codec-encoded event payload]
```

- channel_id 1-127 は 1-byte varint prefix (= hot path 最小 overhead)
- 128-16383 は 2-byte、 以降漸進的に増加 (= max 10 byte for u64::MAX)
- 1 datagram = 1 event message、 chunking / fragmentation 不可、 MTU 超過は `SendDatagramError::TooLarge`

#### Type / API

- **`crate::network::DatagramChannel<C>`** — `UnisonChannel<C>` (= stream channel) と別型分離、 datagram-specific semantics を型レベルで表現
- **`ProtocolServer::register_channel_datagram(name, channel_id, handler)`** — datagram channel handler 登録
- **`ProtocolServer::broadcast(channel_name, event)`** — 全 active connection への best-effort broadcast、 戻り値 = 配送成功 connection 数
- **`ProtocolClient::open_datagram_channel(name, channel_id)`** — datagram channel open (default codec = JsonCodec)
- **`ProtocolClient::open_datagram_channel_with::<C>(name, channel_id)`** — 任意 codec 指定版
- **`ProtocolServer::spawn_listen_shared(self: Arc<Self>)`** — broadcast 用 Arc 保持 spawn (= 既存 `spawn_listen(self)` は委譲、 backward compat)
- **`ProtocolServer::active_connection_count()`** — 現在 active な接続数 (= test / debug 用)

#### Parser / codegen 拡張

- KDL parser: `Channel::backend()` / `Channel::channel_id` / `Channel::validate()` 追加、 `ChannelBackend` enum (`Stream` (default) / `Datagram`) 公開
- Validation: `backend="datagram"` で `channel_id` 未指定 / `channel_id=0` / `request` 混在の 3 ケースで parse error
- Rust codegen: `backend="datagram"` 検出時に `DatagramChannel` 型 + `client.open_datagram_channel(name, channel_id)` build call を出力 (= TypeScript generator は v0.11+ で対応予定)

### 追加 — ProtocolClient connection event hook

`ProtocolClient::subscribe_connection_events()` で connection lifecycle event を subscribe、 server 側 `ProtocolServer::subscribe_connection_events` と parallel な API。 v0.9.0 で発見された軽微 API 非対称の解消。

```rust
let mut rx = client.subscribe_connection_events();
client.connect(url).await?;
loop {
    match rx.recv().await {
        Ok(ClientConnectionEvent::Connected { remote_addr }) => { ... }
        Ok(ClientConnectionEvent::Disconnected { reason }) => {
            // caller がここで自分の reconnect policy で client.connect() を再呼び出し
        }
        Err(_) => break,
    }
}
```

#### API

- **`ClientConnectionEvent` enum**: `Connected { remote_addr }` / `Disconnected { reason }`
- **`ClientConnectionEventReceiver`** (= server 側 `ConnectionEventReceiver` と parallel、 `recv` / `recv_skip_lagged` / `inner` API)
- `tokio::sync::broadcast` capacity 16、 複数 subscriber 対応
- `connect()` 成功時に `Connected` fire + drop detection task spawn
- `disconnect()` 時に explicit `Disconnected` fire (= reason `"explicit disconnect by caller"`)
- QUIC connection drop (= server shutdown / network error) でも自動的に `Disconnected` fire (= `connection.closed().await` driven background task)

#### Auto-reconnect の責務

**Library は auto-reconnect しない**。 caller が `Disconnected` event を見て自身のポリシーで `client.connect(url)` を再呼び出しする責務を持つ。 backoff / circuit breaker / retry budget / jitter / dead letter handling のような戦略は caller の領域 (= chronista-club ecosystem 内で creo-memories は long-lived session 想定、 vantage-point は dashboard refresh 想定、 use case ごとに reconnect 期待値が異なるため)。

### 内部

- `crates/unison-protocol/src/network/datagram_channel.rs` 新規 (= type + varint encode/decode helpers、 LEB128 spec 準拠)
- `crates/unison-protocol/src/network/datagram_dispatcher.rs` 新規 (= per-connection recv loop + `HashMap<channel_id, mpsc::Sender>` dispatch table)
- `crates/unison-protocol/src/network/quic.rs::handle_connection` 拡張 (= dispatcher spawn + handler registration + active connections tracking、 datagram handler 不在の connection では dispatcher を spawn しない (= overhead 回避))
- `crates/unison-protocol/src/network/server.rs` に datagram registry / broadcast / spawn_listen_shared 追加
- `crates/unison-protocol/src/network/client.rs` に subscribe_connection_events / drop detection task / open_datagram_channel 追加
- KDL parser に `ChannelBackend` enum + validate 拡張

### Tests

- unit tests: 198 → **202 passed / 0 failed** (= +4 client event tests)
- integration tests (= `--ignored`): **7 件全 pass** (datagram echo / multi-channel demux / broadcast / connection event 4 種)
- KDL parser tests: +8 (`backend` 属性検証)
- Codegen tests: +2 (`DatagramChannel` 出力 + backward compat)
- clippy clean (`--lib --workspace -- -D warnings`)

### 移行ノート

v0.9.0 → v0.10.0 は **wire 互換 + caller code 互換**:

- 既存 stream channel KDL schema は無改修 (= `backend` 属性なしは `"stream"` default 解釈)
- 既存 `ProtocolClient` / `ProtocolServer` caller は無改修 (= 新規 method は additive、 既存 method の signature 変更なし)
- 既存 stream connection wire format は変更なし (= buffa-encoded packet 形式、 v0.9.0 と完全互換)
- 新規 datagram channel を使う場合のみ:
  1. KDL schema に `backend="datagram" channel_id=N` を追加
  2. codegen を再実行
  3. server 側で `register_channel_datagram` / `broadcast` を呼ぶ
  4. client 側で `open_datagram_channel(name, channel_id)` を呼ぶ

### v0.11+ への引き継ぎ

- **ClientIdentity 概念**: 当初 v0.10.0 scope の 3rd task として候補、 v0.11+ deferred (= mTLS cert subject で 80% 代替可能、 caller use case 確定後に design 議論する healthy path、 memory `mem_1Cb46sKeeZvZSLmdWFgKVU`)
- **Mixed backend channel**: 同 KDL channel に stream + datagram event を共存させる allow 化検討 (= v0.10.0 では strict、 forward-compatible に保持、 spec/02 §8.5 参照)
- **Subscription model**: client が「subscribe」 宣言、 server side が filter で per-client filtering (= broadcast の上位概念)
- **Datagram channel bench 拡充**: channel API 経由の demux overhead 計測、 既存 `benches/datagram.rs` (= connection-level MVP 計測) を channel-level に昇格
- **TypeScript generator の datagram 対応**: v0.10.0 は rust generator のみ拡張、 polyglot SDK 要求が出た時点で TS も対応
- **Auto-reconnect helper layer**: v0.10.0 で「caller 任せ」 を選択、 v0.11+ で opt-in な `client.auto_reconnect_with(BackoffPolicy)` 等の便利層追加検討 (= caller のポリシー奪取は意図的に避け続ける)
- **`WireFormat::supports_datagram() -> bool` flag**: 将来 MessagePack / CBOR 等の wire format pluggable 化と一緒に

## [0.9.0] - 2026-05-15 — 「基盤整備 + buffa pivot」 release

> v0.9.0 のテーマは **「ゴミ無し + wire format pivot + 懸念点全解消」**。 deprecated API 削除、 全 dep の major bump、 dead code / dead dep 掃除、 **wire format を rkyv 0.7 → buffa (Anthropic 製 protobuf) に乗り換え**、 spec/doc 同期を一括で実施。

### 削除 (Breaking)

- **`QuicClient::configure_client()`** — v0.7.0 で `#[deprecated]` 化していた compat wrapper を削除
- **`QuicServer::configure_server()`** — 同上
  - 移行先: `configure_*_with(...)` 明示 API、 もしくは v0.8.0+ Builder API
- **`unison-mcp-probe::ChannelListArgs` / `unison_channel_list` tool** — 「未実装、 サーバ側 meta API が必要」 note のみで実装ゼロだった placeholder を削除
- **workspace dep `bincode`** — `unison-protocol` で宣言されていたが src 内 direct use ゼロの dead dep、 削除
- **workspace dep `rkyv 0.7`** — buffa pivot で完全削除、 `Cargo.toml` / `crates/unison-protocol/Cargo.toml` から remove
- **`crate::packet::Payloadable` trait + `RkyvPayload` / `BytesPayload` / `StringPayload` / `JsonPayload` / `EmptyPayload`** — rkyv 経由の generic payload abstraction を全削除 (= `packet/payload.rs` 廃止)
- **`UnisonPacketHeader::SERIALIZED_SIZE` const** — buffa では header が variable-size になるため fixed const 廃止

### Wire format pivot (Breaking)

v0.8.x までの **rkyv 0.7 archive** から **buffa (Anthropic 製 Protocol Buffers)** に乗り換え。 詳細は [`design/wire-format.md`](https://github.com/chronista-club/club-unison/blob/main/design/wire-format.md) と [`spec/02-unified-channel/SPEC.md`](https://github.com/chronista-club/club-unison/blob/main/spec/02-unified-channel/SPEC.md) §8.4 参照。

#### 旧 wire format (v0.8.x)

```text
[rkyv-encoded UnisonPacketHeader (56 bytes fixed)] [rkyv-encoded payload]
```

#### 新 wire format (v0.9.0+)

```text
[u32 BE header_len] [buffa-encoded PacketHeader] [payload bytes (may be zstd compressed)]
```

#### 主な API 変更

- **`ProtocolMessage`** — 内部 wire を rkyv → buffa に切替、 PascalCase enum / 直 field access の caller API は保持
- **`MessageType`** — `Request` / `Response` / `Event` / `Error` の PascalCase variant は維持 (wire 上は buffa `MessageType` enum の `REQUEST` / `RESPONSE` / `EVENT` / `ERROR` に写像)
- **`UnisonPacket<T: Payloadable>` → `UnisonPacket` (非ジェネリック)** — caller が任意の codec で encode した `Vec<u8>` を渡す形に simplify
- **`crate::proto`** — `proto/protocol.proto` から buffa-codegen された `ProtocolMessage` / `MessageType` / `PacketHeader` + zero-copy `*View` 型を expose
- **wire の binary は v0.8 ↔ v0.9 で互換性なし** — v0.8.x client / server とは接続できない (= 双方 v0.9.0 に揃える必要)

#### Pivot motivation

- **polyglot 親和性**: rkyv は Rust 固有、 buffa は protobuf wire format で多言語 SDK 化が容易
- **schema evolution**: protobuf の field number 互換性で前方/後方互換が取れる
- **Anthropic ecosystem alignment**: buffa は Anthropic 製 protobuf、 club-unison が Claude / Anthropic 周辺 tool との接続を取りやすい
- **rkyv 0.7 → 0.8 移行コスト回避**: 既存 packet 構造で rkyv major bump すると trait bound 地獄、 どうせ redesign するなら buffa pivot で済ませる判断

### 変更 (Breaking)

- **MSRV を Rust 1.93 → 1.95 に bump** — workspace 全体 + CI MSRV job
- **spec/02-unified-channel** を `2.0.0-draft` から `2.0.0 / Stable` に確定
- **dep major bump (10 件)**:
  - `rmcp 0.16 → 1.7` (MCP SDK stable API、 `ServerInfo`/`Implementation` を builder pattern で構築)
  - `webpki-roots 0.26 → 1.0` (Mozilla CA list stable interface)
  - `thiserror 1.0 → 2.0` (improved error formatting)
  - `rcgen 0.13 → 0.14` (`CertifiedKey.key_pair` → `signing_key` field rename 対応)
  - `convert_case 0.6 → 0.11` (codegen 安定化)
  - `buffa / buffa-build 0.2 → 0.5` (Anthropic 製 protobuf、 stable API)
  - `cgp / cgp-component 0.4.2 → 0.7.0` (Context-Generic Programming)
  - `criterion 0.5 → 0.8` (deprecated `criterion::black_box` → `std::hint::black_box` 対応)
  - `kdl 6.3.4 → 6.5.0` (schema 安定化)
- **`cargo update` で transitive dep を 30+ 件 patch / minor 更新** (tokio 1.40 → 1.52、 rustls 0.23.36 → 0.23.40 等)

### 追加 (拡張準備)

- **`proto/protocol.proto`** — buffa wire format core schema (`ProtocolMessage` / `MessageType` / `PacketHeader`)
- **`crate::proto` module** — buffa-codegen 出力 (`$OUT_DIR/protocol.mod.rs`) を `include!` で expose、 main types + zero-copy `*View` + `__buffa::{ext,oneof,view}` まで一括
- **`crate::wire::WireFormat` trait** — wire format pluggable 抽象化 hook (v0.10+ で `MessagePackWire` / `CborWire` 等を追加できる余地)
- **`design/wire-format.md`** — wire format 設計 doc (= living doc)、 v0.9.0 buffa pivot 完了状態を反映、 §5 で v0.10+ 引き継ぎ
- **`spec/02-unified-channel` §8.4** — wire format buffa 段落 (= layout / proto schema / WireFormat trait 拡張 hook)
- **`spec/02-unified-channel` §8.5** — datagram MVP section (= QUIC unreliable / unordered、 ≤MTU、 3DCG transform 大量配信想定)
- **`QuicClient::send_datagram` / `recv_datagram`** — datagram MVP API (= connection-level thin wrapper、 channel 抽象は v0.10+ で `event "X" backend="datagram"` schema 拡張と一緒に統合予定)
- **`benches/ping_pong.rs`** — 1 req/1 resp round-trip latency baseline (payload 16 / 64 / 256 / 1024 B、 「通常の 1 リクエスト・レスポンス」 dogfood)
- **`benches/datagram.rs`** — 3DCG position/rotation 大量配信 baseline (payload 64 = 1 transform / 1300 = MTU max × burst 100 / 1000、 unison MVP API 経由)

### 内部 (ゴミ掃除)

- `unison-agent` の Cargo.toml に `description` / `publish = false` を明示 (意図しない publish 防止)
- `club-unison` の Cargo.toml に `[package.metadata.docs.rs]` 追加 (`all-features = true` + `--cfg docsrs`)
- `.mcp.json` を git track から外す (`.gitignore` 既設定の cache を除去)、 `.gitnexus/` を ignore 追加
- **CI test command 整理**: `cargo test --tests --workspace -- --skip packet` → `cargo test --workspace`
  - 旧 `--skip packet` filter は **そもそも noop** だった (= `--tests` flag が lib unit を除外していたため、 packet 名 inline test は 1 度も走っていなかった)。 撤去 + lib unit を CI に投入。
  - CLAUDE.md / README.md も同期
- `unison-mcp-probe` の `tool_router` field に `#[allow(dead_code)]` (rmcp 1.x macro 経由参照のため dead_code analysis 対象外)
- `unison-agent/src/lib.rs` の docstring example で `AgentClient::new()` の不要な `.await` を削除 (claude-agent-sdk の new は sync)
- benches (`quic_performance` / `throughput`) を `criterion::black_box` deprecated 警告から `std::hint::black_box` に移行
- `CONTRIBUTING.md` の `Tokio 1.40 以上` → `1.52 以上` (workspace dep と整合)、 OpenSSL/BoringSSL 表現を rustls + ring に修正
- `README.md` の `club-unison = "^0.7"` → `"^0.9"`、 v0.7.0 trust model 説明に v0.9.0 削除言及追加
- `CHANGELOG.md` に `[Unreleased]` section 追加 (Keep a Changelog 準拠)

### 移行ノート

下流 (chronista-club ecosystem の caller) は以下に置き換え:

```rust
// 旧 (削除)
let client = QuicClient::configure_client().await?;
let server = QuicServer::configure_server().await?;

// 新 (v0.7+ 明示 API)
let client = QuicClient::configure_client_with(TrustAnchors::SkipVerification).await?;
let server = QuicServer::configure_server_with(CertSource::dev_localhost()).await?;

// もしくは v0.8+ Builder API (推奨)
let client = QuicClient::builder()
    .trust_anchors(TrustAnchors::System)
    .build();
let server = QuicServer::builder(server)
    .cert_source(CertSource::dev_localhost())
    .build();
```

### v0.10+ への引き継ぎ

- `WireFormat` trait に `MessagePackWire` / `CborWire` 等 buffa 以外の具体実装追加
- `ProtocolMessage` を format 非依存に redesign (= buffa decoupling)、 channel negotiation で wire format 選択
- benchmark living doc (= `design/bench-baseline.md`) を CI で auto regen、 release CI 自動化と組み合わせ (team-b dispatch 予定)
- packet module 内の inline test を CI で初実走 (= v0.9.0 で `--skip packet` filter 撤去で初実走、 v0.10+ で coverage 拡大)

## [0.8.2] - 2026-05-15

### 変更
- **GitHub repo を `chronista-club/unison` → `chronista-club/club-unison` に rename**
  - 旧 URL は GitHub の 301 redirect で自動転送、既存参照は壊れない
  - `Cargo.toml` の `homepage` / `repository` を新 URL に更新
  - `README.md` / `SECURITY.md` / `CONTRIBUTING.md` の URL 更新
- 過去の CHANGELOG entry は意図的に旧 URL のまま (歴史的記録)、redirect で機能

### API 影響

なし。crate 名 (`club-unison`) と repo 名が一致したことで discoverability が向上する metadata-only patch。

## [0.8.1] - 2026-05-15

### 修正
- **README の relative link を絶対 URL 化** — crates.io 上で render される際に repo 内の他ファイル/ディレクトリへの相対参照が壊れる問題を解消
  - `CHANGELOG.md` / `LICENSE` / `crates/unison-protocol` / `crates/unison-agent` / `spec/01-core-concept/SPEC.md` / `spec/02-unified-channel/SPEC.md` / `guides/channel-guide.md` の 7 link を `https://github.com/chronista-club/unison/...` に書き換え
- API・実装の変更なし、README のみの patch

## [0.8.0] - 2026-05-15

### 追加
- **`QuicServer::builder(server)`** / **`QuicClient::builder()`** — v0.8.0+ の推奨構築 API
  - `QuicServerBuilder::cert_source(CertSource)` — server 側 cert を明示
  - `QuicClientBuilder::trust_anchors(TrustAnchors)` — client 側 trust を明示
  - 旧 `QuicServer::new()` / `QuicClient::new()` は backward compat 用に維持 (default = `dev_localhost` / `SkipVerification`)
- **`examples/builder_api.rs`** — 4 ユースケース (dev quickstart / internal mesh / from file / public CA) の使用例

### 変更
- `QuicClient` 内部に `trust_anchors: TrustAnchors` フィールド追加、`connect` が builder で設定された値を使用
- `QuicServer` 内部に `cert_source: CertSource` フィールド追加、`bind` が builder で設定された値を使用
- `unison-mcp-probe`: `unison_ping` / `unison_call` tool に `trust` 引数追加 (`"skip"` (default) | `"system"`)
  - builder API のリファレンス実装として機能

### 内部
- 既存 `connect()` / `bind()` は instance の `trust_anchors` / `cert_source` を読むので、builder 経由なら明示的、`new()` 経由なら従来 default で互換性維持
- これにより `ProtocolClient::new_default()` / `QuicClient::new()` 利用者は無変更で v0.8.0 に上がれる

## [0.7.0] - 2026-05-15

### 追加 (新 TLS API)
- **`CertSource` enum** (`network::cert`) — server 側の証明書取得戦略
  - `SelfSigned { subject_alt_names }` — 起動時 self-signed (dev / internal mesh)
  - `Provided { certified_key: Arc<CertifiedKey> }` — 直接渡し (production)、`Arc` で private key の duplication を回避
  - `FromFile { cert_path, key_path }` — k8s secret mount 等の path-based
  - Helper: `CertSource::dev_localhost()` / `CertSource::internal_mesh(sans)`
- **`TrustAnchors` enum** (`network::trust`) — client 側の trust anchor
  - `System` — webpki-roots Mozilla bundle (production)
  - `Custom(Vec<CertificateDer>)` — pinned CA / internal mesh
  - `SkipVerification` — **DEV ONLY**、選択時 `tracing::warn!` 警告
- **`InternalMeshKeypair`** (`network::mesh`) — server cert + client trust anchor のペア生成
  - `InternalMeshKeypair::generate(sans)` で同じ cert material 由来の両半分を取得
- **`QuicServer::configure_server_with(CertSource)`** / **`QuicClient::configure_client_with(TrustAnchors)`** — 明示的 cert/trust 指定

### 削除 (Breaking)
- **build.rs での cert 生成廃止** — `build_certs.rs` 削除、`assets/certs/` ディレクトリ削除
- **`rust-embed` 依存削除** — embed された self-signed cert は配布不可
- **`QuicServer::load_cert_embedded()` 削除** — embed 経路自体が無くなったため
- **`QuicServer::load_cert_auto()` 削除** — 暗黙の fallback chain 廃止、operator 明示選択へ
- `network::quic::SkipServerVerification` (pub) → `network::trust` 内に internal 化

### 非推奨化 (v0.9.0 = 2026-08-15 削除予定)
- `QuicServer::configure_server()` — `configure_server_with(CertSource::dev_localhost())` を呼ぶだけのコンパチ wrapper
- `QuicClient::configure_client()` — `configure_client_with(TrustAnchors::SkipVerification)` を呼ぶだけのコンパチ wrapper

### crates.io publish 解禁
- v0.7.0 で `cargo publish -p club-unison` の verify step (`Source directory was modified`) が通る
  - 原因だった build.rs の `assets/certs/` 書込みを排除
- 初の crates.io 公開 (club-unison v0.7.0)

### 設計原則
- **「Default は不便にする」** — 暗黙の安全でない default を消す
- **「ライブラリは plumbing、operator が trust 決定」** — trust model を library が選ばない
- **「Variant 拡張可能性」** — 将来 `Acme` (Let's Encrypt) / `Pkcs11` 等を variant 追加可能
- 議論記録: creo `mem_1Cb37qLW3Yq1hE7kQmV34a` (+ Moody Blues review annotation `mem_1Cb38UA6WyEd8pKPM4yFsL`)

### Moody Blues review 反映
- Issue 1 (Critical, Score 92): SkipVerification の de-facto default を回避、`SkipVerification` 選択時に `tracing::warn!` 警告
- Issue 2 (High, Score 88): `Provided` は `Arc<rustls::sign::CertifiedKey>` を取り、private key の clone を排除
- Issue 3 (High, Score 82): `InternalMeshKeypair` が server cert + client trust の **ペア**を返す (client 側の穴を塞ぐ)
- Issue 4 (High, Score 79): 旧 API は `#[deprecated]` で残し、v0.9.0 削除予定 sunset date を明記

### 下流影響

下流 (fleetflow / vp / fleetstage):
```toml
club-unison = "0.7"
```

旧 API は deprecation warning が出る。`#[deprecated]` 期限は **2026-08-15 (v0.9.0)**:
```rust
// 旧 (deprecation warning)
let server_config = QuicServer::configure_server().await?;

// 新 (推奨)
use club_unison::network::CertSource;
let server_config = QuicServer::configure_server_with(CertSource::dev_localhost()).await?;
```

## [0.6.0] - 2026-05-15

### 変更 (Breaking)
- **`club-kdl` への依存切替 + lib name 統一**
  - workspace dep: `unison-kdl = { git = ... }` → **`club-kdl = "0.5"`** (crates.io から取得、git dep 廃止)
  - `crates/unison-protocol/Cargo.toml` の `[lib].name`: `unison` → **`club_unison`** (full rename policy 採用)
  - 全 `use unison::...` → **`use club_unison::...`** (40+ 箇所一括置換)
  - 全 `use unison_kdl::...` → **`use club_kdl::...`** (2 箇所)
- workspace 内 dep: `unison = { package = "club-unison", ... }` alias を廃止 → 直接 `club-unison = { path = "..." }` 参照に変更

### 命名規則の確定 (full rename policy)

v0.5.0 では「package name のみ rename、lib name は据置」だったが、v0.6.0 で **「lib name も full rename」** へ方針変更:

| Layer | v0.5.0 (旧方針) | v0.6.0 (新方針) |
|-------|----------------|----------------|
| crates.io package | `club-unison` | `club-unison` |
| lib name (`use`) | `unison` (据置) | **`club_unison`** (rename) |
| directory | `crates/unison-protocol/` | (据置) |

理由: `club-kdl` 側 (lib name `club_kdl` に full rename 採用) と整合性を取るため、本 crate も統一。

### 内部
- `deny.toml`: git source 許可リストから unison-kdl 削除 (crates.io 公開に移行)
- README: dep 例 + 使用例を `club_unison` に更新

### 下流影響

下流 consumer (fleetflow / vp / fleetstage / 等):
```toml
# 旧
club-unison = "0.5"   # use unison::...
# 新 (v0.6.0)
club-unison = "0.6"   # use club_unison::...
```

ソースコードの `use unison::...` も全て `use club_unison::...` に書き換え必須。

### crates.io publish

本リリースで初の crates.io 公開が可能になる (依存 `club-kdl` が crates.io 公開済みのため)。

## [0.5.0] - 2026-05-15

### 変更 (Breaking — Cargo.toml level only)
- **crate を `unison` から `club-unison` に rename** (chronista-club 命名規則に統一)
  - crates.io 上の名前: `unison` → **`club-unison`** (旧名は別人 RobertWHurst の config loader、名前衝突回避)
  - lib name は `unison` で据置 — **ソースコードの `use unison::...` は変更不要**
  - 下流 consumer は Cargo.toml の dep 行のみ更新:
    ```toml
    # 旧
    unison = "0.4"
    # 新
    club-unison = "0.5"
    # または alias 維持
    unison = { package = "club-unison", version = "0.5" }
    ```
- workspace 内の `unison-agent` / `unison-mcp-probe` の `unison` dep は `package = "club-unison"` alias で `use unison::...` を据置

### 内部
- ディレクトリ名は据置 (`crates/unison-protocol/` 等)。package name のみ rename。
- 命名規則の根拠: chronista-club ecosystem の crates.io 公開 crate は **`club-` prefix** で統一 (vs 内部ツール用 `cc-` prefix = ccwire / ccws)

### Future (本リリースの blocker ではないが残課題)
- `unison-kdl` も同様に `club-kdl` に rename 予定 (別 repo 作業)
- `club-kdl` の crates.io 公開後、本 crate も `cargo publish` 可能になる (現状は git dep 依存のため publish 不可)

## [0.4.2] - 2026-05-14

### 修正
- QUIC channel handler の正常 close (EOF) を ERROR から DEBUG に degrade ([#30](https://github.com/chronista-club/unison/pull/30))
  - 正常終端の `NetworkError::Protocol("Channel closed" | "Raw channel closed" | "Request cancelled: channel closed")` が ERROR ログされていた問題を解消
  - fleetstage prod で 24h 5739 件の偽 ERROR ノイズを発生させていた base 要因を除去

### 追加
- `NetworkError::is_normal_close()` helper メソッド
  - 3 種類の正常 channel 終端 (`recv` / `recv_raw` / `request`) を判定
  - 文字列マッチで暫定実装 (将来 `NetworkError::ChannelEof` enum variant 化予定 — USN-5)
- Channel lifecycle ログの対称化: open 側も `debug!` で記録 (close 側と対応)

### 内部
- 設計ヒアリングを Linear に集約 (USN-1〜5)
- Hierophant Green 💚 KDL schema を `schemas/hierophant.kdl` に定義 (USN-3 Phase 1)
- `unison-mcp-probe` crate を追加: Claude Code から Unison サーバを対話的につつく MCP tool 群 (USN-2)

## [0.4.1] - 2026-04-25

### 追加
- QUIC が DNS hostname と IPv4 リテラルを受け付けるように拡張 ([#29](https://github.com/chronista-club/unison/pull/29))
  - `parse_ipv6_address` → `resolve_socket_addr` (async, `tokio::net::lookup_host` ベース)
  - URL scheme strip (`https://` / `http://` / `quic://`)
  - 9 件の unit test 追加 (IPv4 / IPv6 / hostname / scheme / unresolvable)

### 後方互換
- 既存 `[ipv6]:port` / `::1` / `8080` / `localhost:port` 経路は全て維持 (additive)

## [0.4.0] - 2026-04-19

### 追加
- Codec トレイト + buffa (protobuf) 統合
  - `UnisonChannel<C: Codec>` で JSON / protobuf を差し替え可能に
  - `JsonCodec` (`serde::Serialize` / `DeserializeOwned`) と `ProtoCodec` (`buffa::Message`) を提供

## [0.3.0] - 2026-02-20

### 追加
- `ServerHandle`: `spawn_listen()` によるバックグラウンド起動とグレースフルシャットダウン
  - `shutdown()`: グレースフルシャットダウン
  - `is_finished()`: 終了状態の確認
  - `local_addr()`: バインドアドレスの取得
- `ConnectionEvent`: 接続/切断のリアルタイム通知
  - `Connected { remote_addr, context }` / `Disconnected { remote_addr }`
  - `subscribe_connection_events()` で購読
- Raw bytes チャネルサポート: rkyv/zstd をバイパスした最小オーバーヘッドのバイナリ通信
  - `UnisonChannel::send_raw()` / `recv_raw()`
  - Typed Frame フォーマット: `[4B length][1B type tag][payload]`（0x00=Protocol, 0x01=Raw）
- `UnisonStream::send_frame()` / `recv_frame()` / `recv_typed_frame()`: フレームベースの直接 I/O
- `UnisonStream::close_stream()`: `&self` で呼べるストリームクローズ

### 修正
- チャネル通信の二重ラッピングバグを修正（ProtocolMessage が二重にネストされていた）
- `SystemStream::receive()` の `read_to_end` 問題を修正（マルチメッセージ通信が不可能だった）
- `UnisonChannel` のストリーム参照を `Arc<Mutex<UnisonStream>>` → `Arc<UnisonStream>` に簡素化

### 変更
- チャネル内部の送受信を `SystemStream` 経由から直接フレーム I/O に移行
- README.md を v0.2 以降の現状に合わせて全面更新

## [0.2.0] - 2026-02-16

### 追加
- `UnisonChannel`: 統合チャネル型（request/response + event push）
  - `request()`: Request/Response パターン（メッセージID自動生成、pending管理）
  - `send_response()`: サーバー側 Response 送信
  - `send_event()`: 一方向 Event 送信
  - `recv()`: メッセージ受信（Request/Event）
  - 内部 recv ループによる自動振り分け（Response → pending oneshot、その他 → event queue）
- KDL スキーマに `request` / `returns` / `event` 構文を追加
  - `ChannelRequest` / `ChannelEvent` パーサー構造体
- `CLAUDE.md`: プロジェクト開発方針ドキュメント
- Identity Channel: `ServerIdentity` によるリアルタイム自己紹介
- `ConnectionContext`: 接続状態管理（チャネルハンドル、Identity）

### 変更
- **Unified Channel アーキテクチャ**: RPC を全廃し、全通信をチャネルに統一
- `MessageType`: 10 variants → 4 に簡素化（`Request`, `Response`, `Event`, `Error`）
- `ProtocolServer`: `register_handler()` → `register_channel()` に移行
- `ProtocolClient`: `call()` 削除、`open_channel()` → `UnisonChannel` を返す
- KDL スキーマ: `service`/`method` → `channel`/`request`/`event` 構文に移行
- Rust コード生成: `UnisonChannel` ベースに更新
- TypeScript コード生成: `call()` → `request()` に統一
- Examples / Tests / Benchmarks を全て channel ベースに書き換え
- 仕様ドキュメント（spec/01〜03）を Unified Channel に全面書き換え
- 設計ドキュメント（design/）を UnisonChannel アーキテクチャに更新

### 削除
- `register_handler()` / `call()` / `open_typed_channel()` — 旧 RPC メソッド
- `QuicBackedChannel<S, R>` / `StreamSender` / `StreamReceiver` / `BidirectionalChannel` — 未使用型
- `ProtocolClientTrait` / `ProtocolServerTrait` / `UnisonServerExt` / `UnisonClientExt` — 旧トレイト
- `MessageType` の 7 deprecated variants（Stream系）
- `process_message()` / `handle_call()` — 旧 RPC サーバー処理
- `send_response()` (quic.rs 内の dead code)

## [0.1.0-alpha3] - 2025-10-21

### 追加
- 新しい`frame`モジュールの実装
  - `UnisonFrame`構造体でヘッダー、ペイロード、フラグ、設定を統合管理
  - `RkyvPayload`によるゼロコピーシリアライゼーション
  - Zstd圧縮とCRC32チェックサム機能
  - フレームベースの通信プロトコル
- `.claude/skills/developer.md`を追加して開発規約を整理
- `design/packet.md`を追加してパケット仕様を文書化

### 変更
- パーサーをknuffelに完全移行
  - KDLスキーマパーシングをknuffelベースに統一
  - インラインメソッド定義をサポート（`MethodMessage`型）
- ネットワーク層を`UnisonFrame<RkyvPayload<ProtocolMessage>>`を使用するように統合
- `packetモジュールをframeモジュールにリネーム
- テストコードを`new_with_json()`メソッドに統一
- WebSocketモジュールを削除（QUICに集中）

### 改善
- CI/CDの強化
  - Windows環境でのPDB制限エラーを回避（codegen-units増加）
  - macOS環境でのリンカーシンボル長制限に対応
  - Clippy警告を修正してCI通過を実現
- ドキュメント整理
  - 英語版ドキュメントを削除して日本語版に集約
  - 不要なファイルを削除（CONTRIBUTING.ja.md、SECURITY.ja.md等）
- 依存関係の更新
  - MSRV（Minimum Supported Rust Version）を1.85に更新
  - `cargo-deny` 0.18フォーマットに対応
  - knuffelをフォーク版（chronista-club/knuffel）に変更

### 修正
- パケットビルダーでチェックサムが正しく有効化されるように修正
- CI環境でのリンカーエラーを修正
- フォーマットとベンチマークのAPIミスマッチを修正
- スキーマパーステストを簡略化

## [0.1.0] - 2025-01-05

### 追加
- 🎵 QUICトランスポートを採用したUnison Protocolの初期リリース
- 型安全な通信のためのKDLベースのスキーマ定義システム
- 超低遅延トランスポートを備えたQUICクライアントとサーバー実装
- 包括的な型検証とコード生成を備えたスキーマパーサー
- Quinn + rustlsを使用したTLS 1.3対応の最新QUICトランスポート層
- 自動証明書生成とプロダクション用rust-embedサポート
- コアプロトコル型: `UnisonMessage`, `UnisonResponse`, `NetworkError`
- `UnisonClient`, `UnisonServer`, `UnisonServerExt` トレイトによるネットワーク抽象化
- 完全なドキュメントとQUICプロトコル仕様
- 実装例:
  - `unison_ping_server.rs` - ハンドラー登録機能を備えたQUICベースのping-pongサーバー
  - `unison_ping_client.rs` - レイテンシ測定付き高性能QUICクライアント
- スキーマ定義:
  - `unison_core.kdl` - コアUnisonプロトコルスキーマ
  - `ping_pong.kdl` - 複数メソッドを含むping-pongプロトコル例
  - `diarkis_devtools.kdl` - 開発ツール用の高度なプロトコル
- 包括的なテストスイート:
  - `simple_quic_test.rs` - QUIC機能と証明書テスト
  - `quic_integration_test.rs` - 完全なクライアント・サーバー統合テスト
- `build.rs`による自動証明書生成ビルドシステム
- オープンソース配布用MITライセンス

### 機能
- **型安全性**: KDLスキーマによるコンパイル時と実行時のプロトコル検証
- **QUICトランスポート**: TLS 1.3暗号化による超低遅延通信
- **マルチストリームサポート**: 単一接続での効率的な並列通信
- **ゼロコンフィギュレーション**: 開発環境用の自動証明書生成
- **プロダクション対応**: バイナリ内の組み込み証明書用rust-embedサポート
- **スキーマ検証**: 包括的な検証を備えたKDLベースのプロトコル定義
- **コード生成**: 自動クライアント/サーバーコード生成（Rust完成、TypeScript予定）
- **非同期ファースト**: 高性能非同期I/Oとfutures用にtokioで構築
- **包括的テスト**: 完全なクライアント・サーバーシナリオの単一プロセス統合テスト
- **開発者体験**: tracingによるリッチなログ、エラー処理、デバッグサポート

### 技術詳細
- **コア依存関係**: 
  - `quinn` 0.11+ - QUICプロトコル実装
  - `rustls` 0.23+ - ring暗号によるTLS 1.3暗号化
  - `tokio` 1.40+ - フル機能付き非同期ランタイム
  - `kdl` 4.6+ - スキーマ解析と検証
  - `serde` 1.0+ - derive機能付きJSONシリアライゼーション
  - `rcgen` 0.13+ - 自動証明書生成
  - `rust-embed` 8.5+ - バイナリへの証明書埋め込み
  - `Cargo.toml`に完全な依存関係リストと機能
- **ビルドシステム**: 証明書自動生成とコード生成を備えたカスタムビルドスクリプト
- **テスト**: 包括的なユニットテスト、QUIC統合テスト、パフォーマンス検証
- **ドキュメント**: 完全なAPIドキュメント、使用例、QUICプロトコル仕様
- **セキュリティ**: デフォルトでTLS 1.3、自動証明書管理、セキュアなデフォルト設定

### リポジトリ構造
```
unison/
├── .github/workflows/ci.yml    # GitHub Actions CI with Rust matrix testing
├── .gitignore                  # Git ignore rules
├── Cargo.toml                  # Rust package with QUIC dependencies
├── LICENSE                     # MIT License
├── README.md                   # Updated QUIC-focused documentation
├── CHANGELOG.md                # This file
├── build.rs                    # Build script with certificate generation
├── src/                        # Source code
│   ├── lib.rs                  # Library entry point with QUIC exports
│   ├── core/                   # Core protocol types and traits
│   ├── parser/                 # KDL schema parsing with validation
│   ├── codegen/                # Code generation for Rust and TypeScript
│   └── network/                # QUIC implementation
│       ├── mod.rs              # Network traits and error types
│       ├── client.rs           # QUIC client implementation
│       ├── server.rs           # QUIC server with handler registration
│       └── quic.rs             # QUIC transport with Quinn/rustls
├── assets/                     # Build-time generated assets
│   └── certs/                  # Auto-generated QUIC certificates
│       ├── cert.pem            # Server certificate
│       └── private_key.der     # Private key
├── schemas/                    # Protocol schema definitions
│   ├── unison_core.kdl         # Core protocol schema
│   ├── ping_pong.kdl           # Example ping-pong with multiple methods
│   └── diarkis_devtools.kdl    # Advanced development tools protocol
├── tests/                      # Integration tests
│   ├── simple_quic_test.rs     # QUIC functionality tests
│   └── quic_integration_test.rs # Full client-server integration
├── examples/                   # Usage examples
│   ├── unison_ping_server.rs   # QUIC server with handler registration
│   └── unison_ping_client.rs   # QUIC client with performance metrics
└── docs/                       # Documentation
    ├── README.md               # Japanese documentation
    ├── README-en.md            # English documentation  
    └── PROTOCOL_SPEC_ja.md     # QUIC protocol specification
```

### パフォーマンス特性
- **接続**: 超高速接続確立
- **レイテンシ**: 超低遅延通信
- **スループット**: マルチストリーミングによる高スループット
- **セキュリティ**: TLS 1.3暗号化とforward secrecy
- **リソース**: CPU/メモリ使用量の最適化

### 今後の予定（ロードマップ）
- [ ] crates.ioへ `unison` v0.1.0 として公開
- [ ] WebTransport APIサポート付きTypeScript/JavaScriptコード生成
- [ ] aioquic統合によるPythonバインディング
- [ ] quic-go統合によるGoバインディング
- [ ] カスタムバリデータによる拡張スキーマ検証
- [ ] パフォーマンスベンチマークと最適化分析
- [ ] ロードバランシングとコネクションマイグレーション機能
- [ ] 大規模データ転送のためのストリーミングサポート

### 移行に関する注意
これはQUICトランスポートを主要プロトコルとした初期の独立リリースです。このフレームワークは、優れたパフォーマンスとセキュリティ特性を活用し、QUIC通信専用に設計されています。

### 既知の問題
- 本番環境での証明書検証には適切なCA署名済み証明書が必要
- 一部の企業ファイアウォールはQUICに必要なUDPトラフィックをブロックする可能性
- WebTransport APIのサポートはブラウザにより異なる（Chrome 97+、Firefox実験的）

### コミュニティとサポート
- GitHub Issues: バグ報告と機能リクエスト
- GitHub Discussions: コミュニティサポートと質問  
- ドキュメント: `docs/` ディレクトリ内の包括的なガイド
- 例: `examples/` 内の本番対応サーバー/クライアント実装