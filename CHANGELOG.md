# 変更履歴

このプロジェクトの主要な変更はこのファイルに記録されます。

フォーマットは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に基づいており、
このプロジェクトは [セマンティックバージョニング](https://semver.org/lang/ja/) に準拠しています。
## [Unreleased]

## [2.0.0] - 2026-09-06 — 呼び出し元の無い public API と化石 docs の一掃

> 2026-09-05 に workspace 全体を俯瞰し (Explore agent 3 本 + 手動裏取り、 34 項目)、
> 「作ったが誰も呼ばない public API」「実装と食い違う仕様書」「同じことを 2 度書いた実装」
> を一掃した major。 **wire bytes は変わらない** (`tests/fixtures/wire/*.hex` に diff なし、
> TS / Swift / Ruby client との byte 互換は不変) ので、 破壊的なのは Rust の API surface だけ。
> 削除対象は workspace 内に加えて ~/repos 配下の利用 repo (chronista-hub / fleetstage /
> fleetflow / vantage-point / creo-memories / cplp-sound-system 等 83 file) を grep して
> 利用ゼロを確認したものに限り、 外部で使用中だった `ProtocolServer::broadcast` / `MeshCa` /
> `send_raw` / `recv_raw` は残した。 芯 (channel / wire / QUIC runtime) には手を入れていない。

### Migration (1.9 → 2.0)

呼び出し側の変更が要るのは以下。 いずれも機械的に置き換えられる。

| 1.9 | 2.0 |
|---|---|
| `server.listen(addr)` / `spawn_listen` / `spawn_listen_shared` / `spawn_listen_with_cert` / `spawn_listen_shared_with_cert` | `Arc::new(server).listener(addr).spawn()` — cert を載せるなら `.cert(src)` を挟む。 呼び出し側で待つなら `.run()` |
| `ProtocolClient::new_default()` | `ProtocolClient::insecure_localhost()` (中身は同じ。 証明書を検証しないことが名前で分かるようにした) |
| `QuicClient::new()` / `QuicServer::new(server)` | `QuicClient::insecure_localhost()` / `QuicServer::builder(server).build()` |
| `NetworkError::Protocol("channel closed")` の文字列一致 | `NetworkError::ChannelEof(_)` の match、 または `err.is_normal_close()` |
| `SchemaParser::parse` の `Result<_, DynamicError>` | `Result<_, ParseError>` (`Kdl` / `Validation` の 2 variant) |
| `ConnectionEvent::Disconnected { remote_addr }` の分解 | `connection_id` が増えたので `..` を足す |
| `unison::network::quic::{UnisonStream, TypedFrame}` | `unison::network::{UnisonStream, TypedFrame}` |
| `unison::network::quic::{read_typed_frame, write_typed_frame, CHANNEL_ACK_METHOD, FRAME_TYPE_*}` | `unison::network::frame::*` |
| `use unison::prelude::*` | 使う型を明示 import (`unison::{ProtocolClient, ProtocolServer, UnisonChannel, NetworkError, ...}`、 `unison::parser::{SchemaParser, ...}`) |
| `UnisonPacket::builder()...build(payload)` | `UnisonPacket::new(payload)` / `UnisonPacket::with_header(header, payload)` |
| `header.packet_type() -> PacketType` | `-> Result<PacketType, u8>` (未知値は `Err(raw)`) |
| `PacketType::from(u8)` | `PacketType::try_from(u8)` |
| `ProtocolServer::spawn_listen_webtransport(...)` | `WebTransportServer::new(server, cert).bind(addr)` → `start_with_shutdown` |
| `QuicServer::generate_self_signed_cert()` / `load_cert_from_files()` | `CertSource::dev_localhost()` / `CertSource::FromFile { .. }` |
| `unison::codec::proto::creo_sync::*` | 利用側の `.proto` を buffa-build で生成する (本 crate は core wire 型のみ) |
| `unison-agent` crate | `unison-mcp` (MCP bridge) |

client 側 (TS / Swift / Ruby) に変更はない。 Ruby gem の native ext は crates.io の
`club-unison = "1.3"` を pin しているため、 2.0.0 公開後に `"2.0"` へ上げる (別 PR)。

### Removed

- **`unison-agent` crate を削除。** `claude-agent-sdk` の薄い wrapper (`AgentClient` /
  `InteractiveClient`) と、 4 tool 全部が `// TODO` の mock 応答だった `UnisonTools`。
  後者の役割は `unison-mcp` が本実装済み。 README の crate 表からも外した。
- **`packet` の未使用 header field / flag / builder を削除。** `UnisonPacketBuilder`、
  `PacketFlags` の `COMPRESSED` 以外 9 bit と accessor / `Display`、
  `PacketType::{Heartbeat, Handshake, Custom}`、 header の `sequence_number` /
  `stream_id` / `message_id` / `response_to` / `correlation_id` と `is_request` /
  `is_response` / `is_oneway`、 `PacketConfig` の preset 3 種と write-only な `version`、
  `CompressionConfig` の preset 4 種。 いずれも production では常に default 値で wire に
  乗っていなかった。 proto は field 6 / 8-11 を `reserved` にし番号を再利用しない。
  `PacketType` は `From<u8>` から `TryFrom<u8>` (未知値 = `Err(raw)`) に、
  `UnisonPacketHeader::packet_type()` は `Result<PacketType, u8>` に変更。
- **`unison::wire` module (`WireFormat` trait) を削除。** 実装ゼロ・呼び出しゼロ・
  encode/decode method も無い「将来の hook」。 他 format が要る時点で設計する。
- **`parser::TypeRegistry` と `FieldType::{to_rust_type, to_typescript_type}` を削除。**
  codegen は `club-kdl-codegen` に分離済みで、 本 crate 内に caller が無かった。
- **`codec::proto::creo_sync` と `proto/creo_sync.proto` を削除。** creo-memories の
  dogfood schema で、 消費者が test だけなのに published crate の public API に乗っていた。
  `ProtoCodec` のテストは core wire の `proto::ProtocolMessage` / `proto::PacketHeader` を
  題材にするよう書き直し、 buffa 自体を試験していた `tests/test_proto_buffa.rs` は削除。
- **network 層の呼び出し元ゼロ public API を削除。** `QuicServer::generate_self_signed_cert` /
  `load_cert_from_files` / `DEFAULT_CERT_PATH` / `DEFAULT_KEY_PATH` (`cert.rs` の `CertSource`
  と三重複、 参照先 `assets/certs/` は存在しない)、 `ProtocolServer::is_running` /
  `listen_webtransport` / `spawn_listen_webtransport` (WebTransport ingress は
  `WebTransportServer` を直接使う)、 `UnisonStream::new`、 `network::ProtocolError`、
  `frame::read_frame` / `write_frame` (8MB 超の書き込み拒否は生きている
  `write_typed_frame` へ移植)、 `identity::ChannelUpdate` (client 3 言語とも未実装、 src も
  送信していない)、 `ProtocolCache::from_file`。 `dial::rank` は private に。
- **`network::quic` の「後方互換」再公開 shim を削除。** `network::quic::{UnisonStream,
  TypedFrame, read_typed_frame, write_typed_frame, CHANNEL_ACK_METHOD, FRAME_TYPE_*}` は
  `network::{UnisonStream, TypedFrame}` / `network::frame::*` から import する
  (`QuicClient` / `QuicServer` は従来どおり `network::quic::` にある)。
- **`unison::prelude` を削除。** 利用者は workspace 内の test 2 file だけで、 `lib.rs` の
  re-export と 7 symbol 重複、 `NetworkError` が 3 つの名前で到達できていた。 明示 import へ。
- **`examples/test_kdl_parse.rs` を削除。** `unison` を import しない KDL v2 の scratch script。
- **重複 test file を削除。** `tests/test_identity.rs` / `tests/test_identity_quic.rs`
  (`tests/test_integ_identity_flow.rs` が上位互換、 後者は QUIC を含まないのに名前が QUIC)。

- **LOW の dead 群を整理 (俯瞰 LOW)。** `DatagramDispatcher` の public な `unregister` /
  `handler_count` / `shutdown` (`#[allow(dead_code)]` 5 箇所、 runtime 経路で未使用) を削除し、
  inner の test helper は `#[cfg(test)]` に。 no-op skeleton だった `DatagramChannel::close()`
  を削除 (drop で閉じる)。 `dial::race` / `rank` の `my_addrs` 引数 (全 caller が `&[]`、
  TODO の置き場でしかなかった) を削除。 `network::ProtocolFrame` alias は `packet::UnisonPacket`
  に一本化。 `UnisonStream` が保持していた未使用の `connection` field と `from_streams` の
  対応引数を削除 (接続の生存は server / client 側の台帳が担う)。
  `SerializationError::JsonError` (構築箇所なし) を削除。 `unison-mcp` の未使用 `thiserror`
  依存を削除。 orphan だった `schemas/hierophant.kdl`、 closed な `dogfood/`、 設定を typecheck
  するだけで v0.8.0 表記のままだった `examples/builder_api.rs` を削除。 `lib.rs` の crate
  doctest を全行コメントアウトから動く最小例に書き直し。

意図して **残した** もの: `ProtocolServer::broadcast` (server → client の datagram push の唯一の
入口、 `test_medium_datagram_broadcast_to_all_clients` で試験済み)、 `MeshCa` (fleetflow control
plane の private CA として使用中)、 raw-frame 経路 `send_raw` / `recv_raw` (cplp-sound-system の
audio 配信で使用中)。

### Changed

- **`quic.rs` からアドレス解釈を `addr.rs` に切り出した (俯瞰 MEDIUM #19)。**
  接続先文字列の解決 (`[::1]:8080` / `localhost` / `https://host:port` など) と
  SNI 名の導出は QUIC そのものとは独立した文字列の責務。 テストごと移して
  `quic.rs` は 1066 行から 758 行に。 公開 API に変化はない (どちらも crate 内部)。
- **`handle_connection` を責務ごとに分割した (俯瞰 MEDIUM #20)。**
  224 行の 1 関数が「datagram handler 起動 / identity 送信 / Connected 発火 /
  accept ループ / 後始末」の 5 つを抱えていた。 `start_datagram_handlers` /
  `send_identity` / `accept_stream_loop` / `handle_incoming_stream` に分け、
  `handle_connection` 自身は 41 行の筋書きだけになった。 挙動は不変。

- **テストファイル名を層に揃えた (俯瞰 MEDIUM #30)。**
  `test_<layer>_<topic>.rs` に統一し、 `small` (実 I/O なし・常時実行) と
  `medium` (実 QUIC・`#[ignore]`) を名前で見分けられるようにした。 4 つの命名
  スキームが混在し、 `test_integ_*` の 7 件中 4 件は実 QUIC を使わない Small
  だった。 `simple_quic_test.rs` は QUIC 通信を一切せず、 4 本のうち 3 本が
  `#[test]` の付かないヘルパーだったので、 独立した test に分割した上で
  `test_small_cert_trust_config.rs` に改名。 規則は `CLAUDE.md` に記載。
- **doc に残っていた死語 API を実体に合わせた。** `spawn_listen*` /
  `new_default` は 2.0.0 で消えているのに、 テストと bench の doc / expect
  メッセージに 10 箇所残っていた。
- **重複の整理 (俯瞰 MEDIUM #17 / #24 / #27)。** 公開 API の形は変わらない。
  - `QuicServer::start` / `WebTransportServer::start` が
    `start_with_shutdown` の accept ループを丸ごと複製していたのを、
    「発火しない shutdown receiver を渡すだけ」 の 4 行に。 accept ループの
    片肺死対策 (2026-07-13) が 1 箇所にまとまり、 片方だけ直す事故が起きなくなる。
  - `unison.auth` / `unison.discovery` の handler が同じ 5 分岐の recv ループを
    持っていたのを `channel::next_request` helper に集約。 channel の正常終端の扱いと
    未知メッセージへの寛容さ (= forward-compat) が 1 箇所に集まる。
  - `unison-cli` の `ping` / `call` / `sniff` が同じ 7 行の接続 prologue を
    複製していたのを `build_client` / `connect` helper に。

- **CI の clippy を全 target に拡げた (俯瞰 MEDIUM #34)。**
  `--lib` だけが `-D warnings` で `--tests` は `continue-on-error` だったため、
  警告が無期限に溜まっていた (実測 8 件)。 加えて `--bins` / `--benches` は
  一度も lint されていなかった。 `--all-targets -- -D warnings` の 1 ステップに統合し、
  既存の 8 件を解消。 `CLAUDE.md` のコマンドも同じものに更新。
- **Ruby クライアントの CI job を追加 (俯瞰 MEDIUM #34)。**
  TypeScript / Swift には job があり Ruby だけ無かった。 `rake compile` +
  `rake test` を ubuntu で実行する。



- **breaking**: **listen 系 5 メソッドを [`ServerListener`] builder に統合。**
  `listen` / `spawn_listen` / `spawn_listen_shared` / `spawn_listen_with_cert` /
  `spawn_listen_shared_with_cert` は 3 つの直交軸 (`self` か `Arc<Self>` か、 block か
  background か、 cert 明示か既定か) を名前の suffix に畳んでいた。 `listener()` が
  `Arc<Self>` を受けることで `_shared` 軸が消え、 起動方法は terminal method、 cert は
  option になる。

  ```rust
  // 1.x
  server.spawn_listen(addr).await?;
  Arc::clone(&server).spawn_listen_shared(addr).await?;
  server.spawn_listen_with_cert(addr, cert).await?;
  server.listen(addr).await?;

  // 2.0
  Arc::new(server).listener(addr).spawn().await?;   // by-value から
  server.listener(addr).spawn().await?;             // Arc<ProtocolServer> から
  server.listener(addr).cert(cert).spawn().await?;
  Arc::new(server).listener(addr).run().await?;     // block
  ```

- **breaking**: **証明書を検証しない経路を名前で正直にした。** 呼び出し側のコードを
  読んだだけで「ここは検証していない」 と分かるようにする。 挙動は変えていない
  (元から `connect` は SkipVerification 時に loopback 以外を拒否する)。

  | 1.x | 2.0 |
  |---|---|
  | `QuicClient::new()` | `QuicClient::insecure_localhost()` |
  | `ProtocolClient::new_default()` | `ProtocolClient::insecure_localhost()` |
  | `QuicServer::new(server)` | `QuicServer::builder(server).build()` |

- **breaking**: **channel の正常終端を `NetworkError::ChannelEof(ChannelEof)` にした。**
  従来は `Protocol(String)` の中身を `"Channel closed"` 等の文字列と照合して判定して
  おり、 生成側 3 箇所と判定側が定数を共有していなかった (= typo で end-of-stream が
  ERROR ログに化ける)。 判定メソッド [`NetworkError::is_normal_close`] はそのまま残る
  ので、 それを使っている caller は **無改修**。 `Protocol(String)` を直接 match して
  いた場合のみ影響する。

- **breaking**: **`SchemaParser::parse` が `ParseError` を返すようになった** (従来は
  `anyhow::Result`)。 併せて構築箇所の無かった `ParseError::{Type, Generic, Anyhow}` を
  削除し、 残る 2 variant (`Kdl` / `Validation`) を実際に使い分ける。 従来は全ての
  error が `Anyhow` に潰れ、 メッセージが `"Anyhow error: KDL parsing error: ..."` と
  二重 prefix になっていた。



- **breaking**: `ConnectionEvent::{Connected, Disconnected}` に `connection_id: Uuid` を
  追加した。 接続の同定は `remote_addr` ではなくこちらを使う (`remote_addr` は衝突しうる)。
  分割代入で全 field を書いている caller は `..` を足すか `connection_id` を受けること:

  ```rust
  // 1.x
  ConnectionEvent::Disconnected { remote_addr } => { ... }
  // 2.0
  ConnectionEvent::Disconnected { remote_addr, .. } => { ... }
  ```


> **breaking (次は 2.0.0)**: 呼び出し元の無い public API を削除する第 1 弾。 削除対象は
> workspace 内に加えて ~/repos 配下の利用 repo (chronista-hub / fleetstage / fleetflow /
> vantage-point / creo-memories / cplp-sound-system 等 83 file) を grep して利用ゼロを
> 確認したものだけ。 wire bytes は変わらない (`tests/fixtures/wire/*.hex` に diff なし)。

### Added

- **`unison schema-lint` が宣言のない型名を警告するようになった (俯瞰 MEDIUM #28)。**
  `Field::field_type()` は既知の型名に当てはまらないものを全部
  `FieldType::Custom` にする。 Custom は下流で完全に素通しされ、
  `SchemaRegistry::validate_request` の型検査は `true` を返し、 unison-mcp が
  合成する JSON Schema にも型制約が付かない。 つまり `type="strng"` のような
  打ち間違いは、 そのフィールドの型検査を黙って無効化していた。
  `typedef` / `enum` で宣言されていない Custom 名を警告する。 invariant 違反とは
  別枠の警告なので exit code は変えない。 `number` → `float`、
  `array<T>` は未実装構文、 といった直し方の示唆も出す。

### Fixed

- **CI が実 QUIC のテストを一度も走らせていなかった (俯瞰の追加分)。**
  `#[ignore]` 付きの Medium test 65 件 (QUIC lifecycle / identity handshake /
  datagram / mesh trust / Happy Eyeballs dial race / auth / discovery ほか) は
  `cargo test --workspace` では実行されず、 CI に `-- --ignored` のステップが
  無かった。 これらの回帰は CI をすり抜けていた。 test job にステップを追加。
- **Ruby CI job に `protoc` が入っていなかった。**
  `club-unison` の build script (buffa-build) が protoc を要求するため、
  他の Rust job と同じく `protobuf-compiler` を入れる。
- **wire golden test が回帰を検知していなかった (俯瞰 MEDIUM #31)。**
  `test_wire_byte_compat.rs` は 5 つの fixture を毎回 `fs::write` で上書きするだけで、
  一度も assert していなかった。 wire format が変われば golden も黙って追従するため、
  同じ golden を読む TypeScript 側の byte 一致テストも回帰を検知できない状態だった。
  golden との**比較**に変更し、 再生成は `UPDATE_WIRE_FIXTURES=1` を明示したときだけに。
  副次的に `cargo test` が source tree へ書き込まなくなった。


- **`ProtocolClient::open_channel` が接続の read guard を握ったまま server の ack を
  待っていた問題を修正。** `open_bi` → open frame 送信 → `__channel_ack` 受信の 3 つの
  await を guard 越しに行っていたため、 その間 `disconnect()` が write lock を取れず
  待たされていた。 `Connection` を clone して guard を即座に手放す形に変更 (= 同 file の
  `open_datagram_channel_with` と同形)。
- **同一 `remote_addr` の 2 接続が互いを追い出していた問題を修正。** `ProtocolServer` の
  active connection 台帳が `SocketAddr` を key にしていたため、 NAT 越しの再 dial や同一
  host からの raw QUIC + WebTransport 併用で 2 本目の登録が 1 本目を silent に上書きし、
  1 本目の切断が 2 本目を broadcast 配信先から消していた。 key を接続ごとに一意な
  `ConnectionContext::connection_id` (UUID) に変更。 回帰テスト
  `active_connections_are_keyed_per_connection_not_per_addr`。
- **datagram channel handler の task が接続終了後も残っていた問題を修正。**
  `handle_connection` が handler を `tokio::spawn` しっぱなしで `JoinHandle` を捨てていた
  ため、 `recv_event` を待たない handler (= 送信専用 / timer loop) は接続が切れても回り
  続けていた (task leak)。 JoinHandle を保持し、 接続終了時に abort する。 回帰テスト
  `datagram_handler_tasks_stop_when_connection_ends` (修正前は tick が増え続けて FAIL)。

### Documentation

- `design/packet.md` を rkyv 時代 (56 byte 固定 header / `Payloadable` / checksum) の
  記述から現行 buffa wire に書き直し。 `design/wire-format.md` から `WireFormat` 節を除去。
- **docs wave (俯瞰 HIGH #9〜#13)**: 化石になっていた仕様と設計文書を実態に合わせた。
  - `spec/PROTOCOL_SPEC.md` を削除 (索引に無い 2025-01 の orphan。 `service` / `method` 構文、
    `UnisonMessage`、 WebSocket transport など存在しないものを「仕様」として記述していた)。
  - `spec/03-stream-channels/SPEC.md` を `spec/02` に統合して削除。 03 の固有内容 (channel
    routing の `__channel:` / `__channel_ack`、 `UnisonChannel` API、 `ConnectionContext`) を
    02 §1.2 / §5.1 / §5.5 / §5.6 に移し、 「旧 `send`/`recv` 構文をパーサーが認識する」 という
    **事実と異なる** 互換記述 (03 §9、 02 §4.5 / §9.2) を撤去。 02 §3.3 の相関方式を
    実装どおり「Response は Request と同じ `id`」 に修正 (`response_to` は使っていない)。
    02 §4.1 の型表を parser の `FieldType` に一致させ (`number` は builtin ではない、
    `float` / `object` / `map` を追加)、 §6 は「codegen」 から「スキーマの runtime 消費者」 に。
  - `spec/01` §7 の packet 記述を rkyv 56 byte から現行 buffa wire に。 `spec/README` の索引に
    04 を追加、 03 は欠番と明記。
  - `design/architecture.md` を全面書き直し (MSRV 1.93 → `[workspace.package]` 参照、
    version 0.3.1、 rkyv / cgp / miette、 存在しない `core/` `context/` `service.rs`、
    `UnisonClient` / `UnisonServer` / `Service` trait などを実態に置換)。
  - `docs/` を解消: `docs/kdl-to-json-schema.md` → `design/kdl-to-json-schema.md` (設計文書)、
    `docs/review/` → `design/review/` (時点記録)。 参照元 (`mapping.rs` / `schema.rs` /
    `unison-mcp/DEMO.md`) と `design/README` の索引を更新。
  - `README.md` / `README.ja.md` の crate 表に `unison-mcp` / `unison-cli` を追加、
    「rkyv + zstd」 を「buffa + zstd」 に。
  - guides / design の KDL 例にあった `type="number"` (= parser に無く untyped になる) を
    `int` / `float` に修正。 `unison-mcp/README` の「rmcp 2.x」 → 3.x、 TS `index.ts` の
    「empty entry」 コメントを撤去。

## [1.9.0] - 2026-09-01 — 依存の全面棚卸し（buffa 脆弱性 2 件解消 + rmcp 3）+ Swift client の zstd 展開

> 依存 crate を crates.io の最新に対して総点検し、semver 非互換で取り残されていた 4 件
> （buffa / rmcp / club-kdl / sha2）を引き上げたリリース。主目的は buffa 0.5.2 に残っていた
> 脆弱性 2 件の解消で、0.5 系に patch 版が存在せず利用側の `cargo update` では直せないため、
> club-unison の Cargo.toml を上げることが唯一の解消経路だった（creo-memories lane からの
> cross-repo handoff）。あわせて、どの member crate からも参照されていなかった
> `[workspace.dependencies]` エントリ 10 件を削除し、nightly に先行統合済みだった Swift client
> の zstd 受信展開修正（#96）を同梱する。公開 crate は `club-unison` のみで、その公開 API に
> 変更はない。SemVer minor（依存の major 相当 bump を含むため patch にはしない）。

### Security

- **buffa 0.5.2 の脆弱性 2 件を解消**（いずれも severity medium、runtime scope）:
  - **GHSA-9pwq-gcrx-wghh** — `OwnedView` の `Deref` 実装が借用を unsound に `'static` へ
    昇格させ、**Use-After-Free** を引き起こしうる問題。buffa 0.7.0 で `Deref` impl 自体の
    削除により修正。
  - **GHSA-f9qc-qg88-7pq5**（CVE-2026-55407） — `decode_unknown_field` の
    unbounded allocation により、細工した wire 入力で**メモリ枯渇 DoS** を起こしうる問題。
    buffa 0.8.0 で decode 時の element memory limit 導入により修正。

  club-unison は全通信の `ProtocolMessage` を buffa で decode するため、両方とも受信経路に
  効く。`club-unison = "1.x"` で解決している下流 crate は、本リリース後に `cargo update`
  するだけで解消される。

### Changed

- **`buffa` / `buffa-build` を `0.5` → `0.9.1` へ**。0.9.0 は自身の codegen が descriptor set を
  element memory limit で弾くリグレッションを含むため、`"0.9"` ではなく **`"0.9.1"` を下限**
  として指定する。移行コストは実測ゼロ: 0.6〜0.9 の破壊的変更は codegen 出力の形
  （`MessageField` の inline 化、`put_len_delimited_header` の `u64` 化）と手書き `Message`
  impl の trait 面（`WirePayload` の opaque 化、`OwnedView::to_owned_message` の infallible
  化、`write_to` の `EncodeSink` 化）に集中しており、club-unison が触れているのは
  `Message::{encode_to_vec, decode_from_slice}` / `EnumValue` / `__buffa_unknown_fields`
  のみで、いずれも非該当。生成コードは `build.rs` → `OUT_DIR` 方式で毎ビルド再生成される
  ため、「checked-in generated code の再生成が必要」という 0.9.0 最大の移行障壁も構造的に
  該当しない。副次的に buffa 側の性能改善が乗る（singular message field の inline 化、
  UTF-8 検証の `smoothutf8` slack fast path 化、`EncodeSink` によるセグメント zero-copy
  flush）。encode 側に protobuf の 2 GiB サイズ上限が入り超過時は panic するが、unison の
  packet はこの桁に達しない。
- **`rmcp` を `2` → `3.2` へ（unison-mcp）**。MCP の response caching / Tasks 拡張の導入に
  伴う 2 点を移行:
  - `ServerHandler::call_tool` の戻り値が `CallToolResult` から `CallToolResponse`
    （`Complete` / `InputRequired` / `Task` の 3 択）へ。unison bridge の tool は常に同期完了
    するため `CallToolResponse::Complete` に包む。内部ヘルパーの戻り値は `CallToolResult`
    のまま。
  - `ListToolsResult` に `ttl_ms` / `cache_scope` / `result_type` が追加され
    `#[non_exhaustive]` 化。`tools` 以外は `..Default::default()` に委ねる（= 非キャッシュ・
    complete）。今後のフィールド追加でも壊れない形。

  `unison-mcp` は `publish = false` のため、公開 crate `club-unison` の依存ツリーには影響しない。
- **`club-kdl` を `0.8` → `0.12` へ**、**`sha2` を `0.10` → `0.11` へ**。いずれもコード変更なしで
  ビルド・テストとも通過。

### Removed

- **どの member crate からも参照されていなかった `[workspace.dependencies]` エントリ 10 件を
  削除**: `proc-macro2` / `quote` / `syn` / `convert_case`（"Code generation" 節ごと。codegen は
  proc-macro ではなく KDL パーサ経由に移行済みで、残骸だった）、`miette`、`cgp` /
  `cgp-component`（"Context-Generic Programming" 節ごと）、`crc32fast`（packet header に CRC は
  無い）、`indexmap`、`scc`。43 → 33 エントリ。`[workspace.dependencies]` は宣言しただけでは
  ビルドに影響しないため気づきにくいが、依存監査のノイズと「使っているつもり」の誤解を生む。

### Fixed

- **Swift client: zstd 圧縮 packet の受信展開を実装**（#96）。Rust server は 2KB 以上の
  payload を自動で zstd 圧縮する（`packet/mod.rs`）が、Swift client は圧縮 packet を明示
  error にしていたため、小さい `open_ack` は届くのに大きな response / event だけが落ちる、
  という切り分けにくい形で失敗していた（2026-08-15、bikeboy-ladyland fieldd の
  64-entity FieldSnapshot 応答が 3s timeout）。Apple の Compression framework は zstd
  非対応のため facebook/zstd 公式（SPM 対応）を依存に追加し、受信側の展開を実装。
  flag 無しで `compressed_length > 0` の packet は名指しで reject する。回帰テストは
  `PacketZstdTests`（自己圧縮 round-trip + flag 検証）と `FieldLiveTests`
  （`FIELD_LIVE=1`、fieldd 相手の実地 interop）。

### Dependencies

- `buffa` / `buffa-build` 0.5.2 → 0.9.1（推移で `buffa-codegen` / `buffa-descriptor` も、
  `smoothutf8` 0.2.3 を新規追加）
- `rmcp` 2.x → 3.2（unison-mcp のみ、`publish = false`）
- `club-kdl` 0.8 → 0.12
- `sha2` 0.10 → 0.11
- 削除: `proc-macro2` / `quote` / `syn` / `convert_case` / `miette` / `cgp` / `cgp-component` /
  `crc32fast` / `indexmap` / `scc`

  semver 互換の範囲で新しい版が出ている依存（`tokio` 1.53 / `uuid` 1.26 / `bytes` 1.12 /
  `kdl` 6.7 / `hdrhistogram` 7.6 等）は、宣言済みの caret 要件がすでにそれらを許容しており
  `cargo update` で解決されるため、下限の引き上げは行わない（下流の解決を不必要に
  縛らないため）。

## [1.8.0] - 2026-07-15 — KDL request safety hint 属性

> channel 作者が自メソッドの副作用特性（read-only / destructive / idempotent）を KDL schema で
> 宣言し、unison-mcp が MCP `ToolAnnotations` へ写して AI agent が尊重できるようにするリリース
> （#94）。1.7.0 の unison-mcp typed I/O + live bridge 化に続く、KDL → MCP hint パイプラインの
> 拡充。SemVer minor（additive、既存 schema / API は無改修）。

### Added

- **KDL request に safety hint 属性 `readonly` / `destructive` / `idempotent` を追加**:
  channel 作者が自メソッドの副作用特性を KDL schema で宣言し（例: `request "Query"
  readonly=#true idempotent=#true { ... }`）、AI agent 等の consumer が尊重する構図。
  unison-mcp は synthesized tool の `ToolAnnotations`（`readOnlyHint` / `destructiveHint` /
  `idempotentHint`）へそのまま写し、あわせて全 synthesized tool に `openWorldHint: true` を
  付与（= bridge は外部 Unison server と対話する、static tools と同方針）。未宣言の hint は
  set せず MCP client の spec default 解釈に委ねる。`readonly=#true destructive=#true` の
  同時宣言は矛盾として parse 時に validation error。additive（既存 schema は無改修で従来
  どおり動作）。spec/02 §4.4 に Request 属性の節を追記（実装先行だった `description` 属性も
  同時に文書化）。

## [1.7.0] - 2026-07-14 — unison-mcp: rmcp 2 系 + live MCP bridge 化

> unison-mcp を MCP spec 2025-06-18+ 世代へ引き上げるリリース。KDL `returns` からの
> `output_schema` 合成・`structuredContent`・live re-discovery・elicitation（#88/#91/#92、
> team-b Moody Blues ディープレビュー済み）に加え、QUIC accept loop の片肺死修正（#90）と
> Edge(nightly) への CI ゲート整備（#89）を含む。SemVer minor（additive、既存 API 不変）。

### Added

- **unison-mcp: rmcp 2 系機能の還元 — typed I/O + live bridge 化**:
  - KDL `returns` block → MCP `Tool.output_schema` 合成。synthesized tools が入出力とも
    typed になり、client が `structuredContent` を検証できる（input と同一 converter =
    `mapping::fields_to_object_schema`、型対応が入出力で完全一致）。
  - 全 tool の結果を `structuredContent` で返却（text content は互換 mirror）。synthesized
    tools は response そのものを structured で返し、output_schema と形が一致する。
  - static tools に `ToolAnnotations`（ping/discover = read-only + idempotent、全 tool
    open-world）、synthesized tools に `title`（= sanitize 前の `channel.method` 表示名）。
  - **live re-discovery**: `unison_discover` が default endpoint への成功時に bridge の
    discovery を置き換え、synthesized tool set を MCP session 中に更新（= server の schema
    進化に追従）。protocol hash 変化時は `notifications/tools/list_changed` を発行
    （`listChanged` capability 宣言済み）。`UnisonBridge.discovered` は `RwLock` 化。
  - **endpoint elicitation**: endpoint が config にも tool arg にも無い場合、elicitation
    対応 client にはその場で接続先を質問（`Peer::elicit`、rmcp feature `elicitation` +
    `schemars` を追加）。非対応/拒否/失敗は従来どおり invalid_request エラー。

### Changed

- **unison-mcp: rmcp を最新 2 系（2.2.0）へ更新**（`1.7` → `2`）: MCP 公式 Rust SDK を 2 系に追従。
  破壊的変更は content 型のみ — `Content`（= `Annotated<RawContent>`）→ **`ContentBlock`**（enum 直、
  `.raw` ラッパ廃止）にリネーム。`Content::text` → `ContentBlock::text`、テストの `c.raw` /
  `RawContent::Text` → `c` / `ContentBlock::Text` に追従。`Tool::new` / `ServerHandler` / `ErrorData` /
  `schema_for_type` の API は不変。
- **unison-mcp: 未使用の rmcp `macros` feature を削除**: `#[tool]` / `#[tool_router]` マクロは撤去済で
  `ServerHandler` を手動 impl しているため、`macros` は dead weight だった。rmcp の `default`
  （`base64` / `macros` / `server`）に `macros` が含まれるため `default-features = false` にして
  `server` / `transport-io` / `base64` のみを明示列挙し、`rmcp-macros` proc-macro を依存グラフから
  除去（ビルド短縮・Minimum を保つ）。

  いずれも `unison-mcp`（`publish = false` の MCP bridge crate）内に閉じ、公開 `club-unison` API への
  影響はなし。
- **unison-mcp: tool description / arg schema / エラーの全面 UX 整備**: description の stale 解消
  （ping の "success message"、trust default の実挙動）、arg schema doc の英語統一（schemars 経由で
  LLM に露出するため）、actionable エラー（文脈 + 次のアクション提示）、instructions の再構成、
  static tools への title 付与。Moody Blues review 指摘の解消（`fields_to_object_schema` の
  field 数 cap + hostile field 名 skip + description sanitize、テスト空白の充填 = unit 45 / E2E 7）。

### Fixed

- **QUIC accept loop の片肺死を根治**: accept loop 内の handshake await が失敗すると loop 全体が
  死ぬ問題を修正。handshake を spawn 済み task 内へ移動し、失敗した handshake が acceptor を
  殺さないようにした。回帰テスト `acceptor_survives_failed_handshake` 追加。

## [1.6.0] - 2026-07-11 — connect_race: Happy Eyeballs v2 staggered-race dialer

> 複数 direct 候補への接続を逐次フォールバックでなく 1 本の時間差レース（RFC 8305 型）に畳む
> dialer（ADR-020 §S6 の consume 側）。direct-first-cut = IPv6 GUA direct のみ、relay fallback
> は次段（`Transport` 抽象）。SemVer minor（additive・opt-in、既存 `connect` API 不変）。
>
> 設計: `design/happy-eyeballs-dial.md`（SSOT）

### Added

- **connect_race — Happy Eyeballs v2 staggered-race dialer**（`network::dial` / `network::client` /
  `network::quic`、ADR-020 §S6）: 複数の direct 候補へ *逐次フォールバック* でなく
  **1 本の時間差レース**で接続し、最初に握手完了した経路を採用・残りを cancel する dialer。
  per-endpoint timeout の「短すぎ＝良経路誤棄／長すぎ＝数秒待ち」ジレンマを、有界な stagger
  だけで構造的に解消する（死経路コスト = stagger 1 tick、「全滅」判定は不要）。
  - `network::dial::race` / `rank`: network 非依存の generic engine。実 I/O は `attempt` closure に
    閉じ、並行タイミングの状態機械を**仮想時間**（`start_paused = true`）で決定論的にテスト
    （unit 10 件）。eager relay arm（握手済 relay を `relay_handicap` まで hold し direct に勝機を
    与える → direct 失敗時の failover +0 RTT）を doctrine default に持つ。
  - `ProtocolClient::connect_race(addrs, server_name, cfg)` / `QuicClient::connect_race`:
    1 個の client Endpoint から IPv6 GUA 候補を staggered race。成功後は `connect` と同じ状態
    （identity / イベント / datagram）で使える。IPv4 は §D3 で deferred のため warn して skip、
    relay fallback は次段（`Transport` 抽象）。
  - `RaceCfg { stagger, relay_handicap, overall_deadline }`: レースのチューニング
    （default = eager relay arm）。

  設計 SSOT: [`design/happy-eyeballs-dial.md`](design/happy-eyeballs-dial.md)。
  SemVer minor（additive・opt-in、既存 `connect` API は不変）。

## [1.5.0] - 2026-06-28 — server-initiated reliable stream (`ServerToClient` を起こす)

> server 起点で connected client へ **reliable・同順** な stream を開く primitive を追加。
> 既に型・KDL (`from="server"`) に宣言されながら runtime で無視されていた
> `ChannelDirection::ServerToClient` を本物にする。reliable は新しい配送 mode ではなく、
> **client 側の受信 handler を server 側と対称化**（どちらも raw `UnisonStream` を直読）して
> 構造的に得る — recv ループ／中継 mpsc を挟まないので、遅い consumer には QUIC flow-control
> backpressure が掛かり、取りこぼしも OOM も起きない。chronista-hub federation relay
> (ADR-020 §S4) の substrate floor。SemVer minor (= additive、opt-in、既存 API 不変)。
>
> 設計: `design/server-initiated-stream.md`（SSOT）

### Added

- **server-initiated reliable stream** (`network::context` / `network::client` / `network::dispatch`):
  server が connected client へ取りこぼし無く push できる stream primitive。
  - `ConnectionContext::open_server_stream(channel) -> UnisonStream`: server 起点で双方向
    stream を開き、先頭に channel 宣言 frame (= `__identity` と同形) を 1 本書く。以降の
    payload は返した raw `UnisonStream` で授受する。stream は persistent (`finish()` しない)。
    既存の `UnisonConn::open_bi()` を再利用し、新しい transport 動詞は増やさない。
  - `ProtocolClient::register_server_channel(channel, handler)`: client が `from="server"`
    channel の handler を登録。handler は raw `UnisonStream` を受け取り**直読**する
    (= server 側 `register_channel` handler と対称)。`connect` 前に登録する。
  - `ConnectionContext::set_conn`: `handle_connection` が server 側でのみ接続を ctx に渡す
    (1 行注入)。client 側 ctx は conn 未 set のままで、`open_server_stream` は誤用 error。

### Changed

- `client_accept_bi_loop`: server 発信 stream を先頭 frame の method で振り分けるよう一般化。
  `__identity` は従来どおり専用 oneshot、それ以外は server-channel registry を引いて handler へ
  `UnisonStream` を渡す。**未登録 channel は従来どおり drop + warn**（後方互換 = 無回帰）。

### Notes

- **完全性 (no-drop) の根拠**: dedicated QUIC stream を直読すると、handler が読まない間に
  受信 window が埋まり送信側が QUIC flow-control で throttle される。完全性は queue
  (bounded→drop / unbounded→OOM) でなく **end-to-end backpressure** が生む。
- **scope 外 (剃刀)**: `get_connection_by_principal` 等の lookup table / relay 専用 API は
  substrate に置かない (利用側 = hub が `wld_id→ctx` map を持つ)。aggregate (多重相関) と
  既存 `UnisonChannel` の global reliable 化は consumer が出るまで保留。
- **制約**: client 受信経路は raw QUIC 専用 (`client_accept_bi_loop` が `quinn::Connection`)
  のため、WebTransport client は server-initiated channel を受けられない (native client は可)。
  `build_identity()` の `from="server"` honor は direction source 整備が前提のため本リリースでは
  未対応 (schema↔runtime 齟齬は残置、二次的)。

## [1.4.0] - 2026-06-27 — connection-level auth primitive (mechanism/policy 分離)

> 全エンドポイント間通信 (federation worlds channel / 連邦 wire / live streaming) の認証を
> connection 確立時に1回行う primitive を追加。authN を connection に、authZ を per-message
> に分離し per-frame 0 bytes。mechanism (= club-unison) と policy (= app の verifier) を分離
> し、library は特定の認証エコシステムに依存しない (OSS ecosystem-neutral)。SemVer minor
> (= additive、opt-in、既存 API 不変・非破壊)。

### Added

- **connection-level auth primitive** (`network::auth` — new): 全エンドポイント間通信
  (federation worlds channel / 連邦 wire / live streaming) の認証を **connection 確立時に
  1 回** 行う primitive。authN を connection に、authZ を per-message (`ctx.principal()` を
  引く app 側 gate) に分離し、**per-frame に auth byte 0**。live streaming の小フレーム
  fan-out / datagram を auth コストで殺さないための設計。SemVer minor (= additive、opt-in)。
  - `ProtocolServer::enable_auth(verifier)`: `enable_discovery` と同型の opt-in API。
    reserved channel `unison.auth` を登録。verifier (= app 注入の policy) は async
    (`Fn(Vec<u8>) -> Future<Option<Principal>>`)。
  - `ProtocolClient::connect_with_credential(url, credential)`: 接続直後に credential を
    1 回提示して認証する helper。
  - `ConnectionContext::{set_principal, principal}` + `Principal = Arc<dyn Any + Send + Sync>`:
    認証済み client の **opaque** な principal を connection に保持。app が downcast する。
  - **mechanism / policy 分離** (`cert::CertSource` 哲学の踏襲): library は credential /
    principal の中身 (Creo ID JWT 等) を一切解釈しない → 特定の認証エコシステムに依存
    しない (= OSS として ecosystem-neutral)。`enable_auth` を呼ばない server は従来通り
    (非破壊)。
  - 設計: `design/connection-auth.md` / E2E: `tests/test_integ_auth.rs` (4 ケース)。

## [1.3.0] - 2026-06-25 — raw QUIC ALPN "unison" + Swift client SDK

> Apple `NWProtocolQUIC` との interop のため raw QUIC に ALPN を追加し
> (RFC 9001 §8.1 — QUIC は ALPN 必須)、Swift native client SDK (`clients/swift`)
> を新設。SemVer minor (= additive)。
>
> ⚠️ **互換性の訂正**（当初の「後方互換」記述は誤り）: ALPN を設定した server は
> ALPN を出さない**旧 raw QUIC client を `no_application_protocol` で拒否する**
> (QUIC は ALPN 必須で、rustls/quinn は plain TLS と違い handshake 時に enforce
> する — empty-ALPN client での実測で確認)。raw QUIC client（Rust / Ruby FFI）は
> **club-unison 1.3.0+ にビルドし直して `"unison"` ALPN を送る必要がある**。
> WebTransport（TS）は HTTP/3 の `"h3"` 別 ingress なので無影響。詳細は
> `design/quic-runtime.md` の ALPN 節。

### Added

- **raw QUIC ALPN** (`network::UNISON_ALPN = "unison"`): server (`quic.rs`) /
  client (`trust.rs` 全 trust mode) が ALPN を negotiate。空 ALPN で handshake
  していた従来は QUIC 仕様逸脱で、Apple `NWProtocolQUIC` 等の厳格実装と interop
  できなかった。WebTransport 経路は HTTP/3 の `"h3"` 固定 (別 ingress) で無影響。
  PR: [#74](https://github.com/chronista-club/club-unison/pull/74)
- **Swift client SDK** (`clients/swift` — new): Apple `Network.framework`
  (`NWProtocolQUIC`) + `swift-protobuf`。`clients/{ruby,typescript}` の swift
  sibling。stream channel (connect / openChannel / request-response / event /
  identity handshake) が実 quinn server 相手に live e2e 済み。API は
  `design/typescript-client-api.md` と同形式 (`AsyncStream` / `async throws` /
  `actor`)。PR: [#75](https://github.com/chronista-club/club-unison/pull/75)–[#78](https://github.com/chronista-club/club-unison/pull/78)

### Notes

- Swift client の datagram channel / `Endpoint.bonjour` discovery / KDL→Swift
  codegen は後続。現状 stream channel が GA。

## [1.2.0] - 2026-06-18 — ProtocolServer cert 指定 spawn (federation API gap)

> chronista-hub の world federation 要件で判明した API gap の解消。spawn 経路が
> `dev_localhost` cert 固定で非 loopback 公開 (tailnet / public federation) が
> できなかった問題を、cert 指定 spawn variant の追加で解決。SemVer minor (= additive)。

### Added

- `ProtocolServer::spawn_listen_with_cert` / `spawn_listen_shared_with_cert`:
  `CertSource` を受ける spawn variant。`QuicServer::builder().cert_source()` を
  経由し、非 loopback アドレスでの公開が可能に。既存 `spawn_listen` /
  `spawn_listen_shared` は `dev_localhost` 既定へ委譲し後方互換維持。
  PR: [#73](https://github.com/chronista-club/club-unison/pull/73)

## [1.1.0] - 2026-05-28 — Hailing α: runtime protocol discovery + AI-native MCP tools

> Hailing α Epic の minor release。 `unison.discovery` channel で server が自身の
> protocol KDL を runtime 配信、 client は `DynamicProtocol::fetch` で typed channel
> を open、 `unison-mcp` bridge が AI agent に typed tools を expose する。
> SemVer minor (= additive)、 dogfood phase。
>
> PR: [#66](https://github.com/chronista-club/club-unison/pull/66)

### Added (Hailing α Epic — 2026-05-27 ~ 2026-05-28)

- **Runtime schema discovery**: `unison.discovery` channel (= `GetProtocol` request + `SchemaUpdated` event)、 server が自身の protocol KDL + version + SHA-256 hash + codecs を runtime 配信
- **Server side** (`crates/unison-protocol/src/network/discovery.rs` + `protocol_cache.rs`): `ProtocolServer::enable_discovery(kdl)` extension method
- **Client side** (`crates/unison-protocol/src/network/schema_registry.rs` + `dynamic.rs`): `SchemaRegistry` (= KDL → runtime channel lookup + validation)、 `DynamicProtocol::fetch` (= discovery 経由で schema を fetch して typed channel を open)、 `DynamicChannel::request` (= payload を schema に validate → fail-fast → server 送信)
- **MCP bridge** (`crates/unison-mcp` — new crate): stdio MCP server + 3 static escape hatch tools (`unison_ping` / `unison_call` / `unison_discover`) + synthesized typed tools (`unison_<channel>_<method>` × N、 KDL の各 `channel.request` から動的合成)
- **KDL → JSON Schema converter** (`crates/unison-mcp/src/mapping.rs`): MCP `Tool.input_schema` / Anthropic Messages API `tools[].input_schema` 共通の output、 docs `docs/kdl-to-json-schema.md` 完備
- **KDL syntax extension**:
  - `field "x" type="array"` → `FieldType::Array(Json)` (= live、 以前は Custom 扱いの dead code)
  - `field "x" type="map"` → `FieldType::Map(String, Json)` (= 同上)
  - `request "X" description="..."` (= MCP tool description の入口、 LLM tool-selection 精度改善)
- **Spec**: `spec/04-discovery/SPEC.md` (= channel KDL、 message flow、 bootstrap codec、 schema hash、 ServerIdentity との補完関係)
- **Demo**: `crates/unison-protocol/examples/hailing_demo_server.rs` (= 4 channels with handlers)、 `crates/unison-mcp/DEMO.md` (= Claude Code を driver にした手順)

### Removed

- **`crates/unison-mcp-probe`** crate を削除 (= 後継 `unison-mcp` が機能を完全 subsume、 probe の `unison_channel_list` TODO は `unison_discover` + server 側 `enable_discovery` で埋まった、 chronista 「Legacy は残さない」 規律)
- `.mcp.json` の `unison-probe` entry を `unison` (= `unison-mcp` binary) に置換

### Dependencies

- `sha2 = "0.10"` を workspace dep に追加 (= `unison.discovery` channel の KDL SHA-256 hash 用)


## [1.0.0] - 2026-05-23 — v1.0 GA: protocol API freeze

> v1.0 sprint の到達点。`alpha.1` → `alpha.2` → `rc.1` 〜 `rc.5` で固めた内容を
> そのまま stable に昇格し、**SemVer の stability commitment** に入る。
> これ以降の minor / patch リリースは API 互換を保つ。破壊的変更が必要な
> 場合は v2.0。

### API freeze 対象（SemVer 互換の保証範囲）

- `unison::{ProtocolClient, ProtocolServer, UnisonChannel, ServerHandle}` の public API
- `network::trust::{TrustAnchors, ...}` および `network::cert::{CertSource, ...}` の trust 抽象
- `network::mesh::{InternalMeshKeypair, MeshCa}` の internal mesh primitive
- `network::quic::{QuicClient, QuicServer, UnisonStream, TypedFrame}`
- `codec::{JsonCodec, ProtoCodec, Codec, Encodable, Decodable}`
- `wire` の proto3 `ProtocolMessage` フォーマット（同一 wire 形式互換）
- KDL channel schema 構文（`channel` / `request` / `returns` / `event` / `field`）

### コード

rc.5 と**同一バイナリ**（version メタデータと CHANGELOG のみ変更）。crates.io 上で stable として認識され、`/crates/club-unison` の landing が v1.0.0 になる。

### v0.x → v1.0 累積（rc.1 entry の要約）

- Polyglot client base: Rust core + TS SDK + Ruby gem
- Unified Channel: request/response + event push + Datagram、JsonCodec / ProtoCodec
- WebTransport ingress（browser ↔ Rust server）
- MeshCa private-CA primitive（rc.3〜）— IPv4 / IPv6 両対応
- `unison` CLI: `ping` / `sniff` / `mock` / `call` / `schema-lint`
- dogfood signal: fleetflow handoff (MeshCa) / VP dashboard 統合 / Ruby client E2E + bench

### Polyglot clients shipped to public registries (2026-05-25)

- **TS SDK**: [`@chronista-club/unison-client@1.0.0`](https://www.npmjs.com/package/@chronista-club/unison-client) → npm (`--tag latest`)
  - 直前: `1.0.0-rc.1` (2026-05-17、`--tag next`)
- **Ruby gem**: [`unison-client@0.1.0`](https://rubygems.org/gems/unison-client) → rubygems（初公開）
  - gem version は `0.1.0` のまま — protocol が 1.0 で freeze している一方、gem 側 client API は scaffold stage を抜けるまで独立して stabilize する方針

## [1.0.0-rc.5] - 2026-05-23 — IPv4 / IPv6 両対応の文書訂正 + crates.io readme refresh

> rc.4 と同じコードのまま、IPv4 / IPv6 両対応を明示するドキュメント訂正を反映した
> readme で再 publish。rc.4 publish 時点では README が「IPv6 専用設計」と誤明言
> しており、crates.io 上の readme もその古い記述で焼かれていた。

### 修正 — IPv4 / IPv6 両対応を明示（誤記訂正）

実装は当初から IPv4 / IPv6 両家族に対応していた（`network::quic::resolve_socket_addr` が両者を受理し、client は target family に合わせて local bind を切り替え、`server.rs` の bind テストも `127.0.0.1` 使用）が、README が「IPv6 専用設計」と誤って明言していたため訂正:

- `README.md` 「IPv6 専用設計」→ 「IPv4 / IPv6 どちらでも繋がる」（bare port → `::1` フォールバックのバイアスは明記）
- `unison` CLI の `ping` / `sniff` / `call` arg help に `quic://127.0.0.1:7878` の IPv4 例を併記
- `spec/01-core-concept/SPEC.md` シーケンス図の note を「IPv6 アドレス解析」→「IPv4 / IPv6 アドレス解析」、`Endpoint::client` を family-matched bind 表記に

ライブラリの API・実装は **rc.4 と同一**（README/SPEC/CLI doc-comment / version メタデータのみ）。

## [1.0.0-rc.4] - 2026-05-23 — crates.io readme refresh + README clone fix

> rc.3 と同じコードのまま、**crates.io 上の readme を同期するための republish**。
> rc.3 を publish した commit には README sync (PR #57) がまだ含まれておらず、
> crates.io の readme が `club-unison = "^0.10"` / `MeshCa` 未掲載の古い版に
> なっていた。crates.io は publish 時点の readme を永続的に焼き込む（後から
> 更新不可、再 publish も不可）ため、readme を更新するには新しい version での
> publish が必要。

### 修正

- README: `git clone` 後の `cd unison` を `cd club-unison`（リポジトリ名と一致）に修正
- crates.io 上の readme を最新版（`MeshCa` 例 + `club-unison = "1.0.0-rc.4"` 表記）で再公開

ライブラリの API・実装は **rc.3 と同一**（コード変更なし、メタデータと README のみ）。

## [1.0.0-rc.3] - 2026-05-22 — MeshCa private-CA primitive + Ruby client 拡充

> rc.2 以降の追補。`unison` trust に private-CA primitive を新設（fleetflow からの
> dogfood handoff 発）、ほか Ruby client のテスト・ベンチと依存更新。

### 追加 — trust: `MeshCa` private-CA primitive

- `network::mesh::MeshCa` — 内部 mesh 用の private 認証局。`generate()` で CA 生成、`issue(sans)` で per-server leaf cert を CA 署名して発行（`CertSource`）、`trust_anchors()` で client 用 `TrustAnchors::Custom([CA])`、`to_pem` / `from_pem` で永続化。
- `InternalMeshKeypair`（self-signed 1 枚の pairwise）が N server にスケールしない問題を解消 — client は CA 1 枚を信頼するだけ（O(1)）、server 追加で client 無改修、per-server key で compromise 隔離。
- client 側は新規コード不要（`TrustAnchors::Custom` が rustls の chain 検証に乗る）。fleetflow の Podman+Quadlet epic の dogfood handoff 発。

### 追加 — Ruby client のテスト・ベンチマーク

- `clients/ruby/test/e2e/` — `unison mock` を subprocess 起動する実サーバ E2E テスト（#52）
- `clients/ruby/bench/` — `Channel#request` の RTT/throughput と GVL 解放効果を計測するベンチマーク（structured-log KDL 形式、#54）

### 変更 — 依存

- `club-kdl` を `0.5` → `0.8` に更新（#53）。club-unison は KDL パース（`from_str` / `KdlDeserialize`）にのみ使用しており API 互換、呼び出し側の変更なし
- `rcgen` の `x509-parser` feature を有効化（`MeshCa::from_pem` が CA cert PEM から `Issuer` を復元するのに必要）

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