# UnisonClient (Swift)

Unison protocol の **Swift client SDK**。`clients/ruby` / `clients/typescript` の swift sibling で、wire-format conformance / protocol semantics / version 互換は club-unison framework 側が owns する。consumer (例: Vantage Point の macOS menu bar agent / visionOS app) はこの package を SPM 依存として使う。

> **Status**: `alpha` — stream channel が実 QUIC で動作(connect / openChannel / request-response / event / identity、`unison mock` 相手の live e2e 済み)。残り: datagram channel / bonjour discovery。下記「実装状況」参照。

## アーキテクチャ

- **transport**: Apple `Network.framework` の `NWProtocolQUIC`(生 QUIC: streams + datagrams, TLS1.3 込み)。quinn server と RFC9000 interop。
- **ALPN**: `"unison"` 固定 = Rust 側 `network::UNISON_ALPN` と一致(QUIC は RFC 9001 §8.1 で ALPN 必須。Apple `NWProtocolQUIC` は ALPN を強制するため、server 側の ALPN 設定が前提 → PR #74 で対応済み)。
- **wire**: `crates/unison-protocol/proto/protocol.proto` → `swift-protobuf`(Apple 公式)で生成(`Sources/UnisonClient/Wire/Generated/protocol.pb.swift`)。
- **framing**: 4B BE length-prefix(stream channel)+ `__channel:` mux + identity handshake(後続 pass)。

## API contract

`design/typescript-client-api.md` と同形式・同体験(ideal-caller-first)。idiom は `AsyncIterable`→`AsyncStream`、`Promise`→`async throws`、生成 `ChannelMeta`→Swift generic + 生成型。

```swift
import UnisonClient

let conn = try await UnisonClient.connect(
    to: .localDaemon(port: 7878),   // or .host("example.com", port: 443) / .bonjour(_)
    trust: .skipVerify              // or .system / .pinned(certDER)
)

// 接続の状態を観測 (reconnect は caller 責務)。 server 側から切れた場合も .disconnected が出る
for await event in conn.connectionEvents { /* .connected / .disconnected(reason:) */ }

let channel = try await conn.openChannel(SomeStreamChannelMeta())   // 既定の期限 5 秒
let response = try await channel.request(SomeRequest(...))          // 既定の期限 30 秒
let slow = try await channel.request(SlowRequest(...), timeout: .seconds(120))  // nil で期限なし
for await event in channel.events { /* server → client push */ }
await channel.close()

await conn.disconnect()
```

### 待ちと後始末

- **待ちは必ず戻る**。`connect` / `openChannel` / `request` / `serverIdentity` / `events` は、応答・期限（`UnisonError.timeout`）・Task の cancel（`CancellationError`）・接続や stream の終わりのどれかで戻る。
- **`.disconnected(reason:)`** は `disconnect()` のときだけでなく、server が接続を閉じたとき・経路が失敗したときにも 1 回出る。server が接続を閉じてから終わればすぐ、黙って消えた（crash / kill）場合は QUIC の idle timeout（最大 30 秒）で検知する。もっと早く知りたければ app 側の心拍を併用する。
- **`disconnect()` は開いた stream を全部閉じる**。閉じ忘れた channel があっても UDP socket は残らない。`disconnect()` / `close()` を呼ばずに手放した場合も deinit で閉じるが、明示的に呼ぶほうが確実。
- **event は 256 件まで溜める**（Rust と同じ）。読まずに放置すると、それ以降に届いた分は捨てられる。`events` の loop を `break` しても channel は開いたままで、もう一度回せば続きから読める。

channel meta は consumer が定義する(KDL schema → Swift codegen は将来、当面手書き):

```swift
struct PingPongMeta: StreamChannelMeta {
    static let name = "ping-pong"
    struct Tick: Sendable {}
    typealias Event = Tick
}
struct Ping: UnisonRequest {
    static let method = "Ping"
    struct Pong: Sendable { let reply: String }
    typealias Response = Pong
    let message: String
}
```

> handoff sketch の `request<R: M.Request>` は、Swift で associatedtype を generic constraint に使えない制約のため `request<R: UnisonRequest>` に置き換えている(caller 体験は不変)。

## 実装状況

| 層 | 状態 |
|----|------|
| Package / 公開 API surface | ✅ |
| wire 型(swift-protobuf 生成) | ✅ |
| QUIC transport / handshake(ALPN "unison" + trust policy) | ✅ (`NWProtocolQUIC`、spike で quinn 疎通実証済み) |
| wire framing(typed frame / UnisonPacket / ProtocolMessage encode・decode）| ✅ (Rust golden fixture と **byte 一致**検証済み) |
| `FrameReader`（chunk 跨ぎ frame 再構成） | ✅ |
| `ChannelStream` / `ChannelTransport` 抽象 | ✅ (transport をロジックから分離) |
| `__channel:` mux（open frame → open_ack 待ち） | ✅ (in-memory test 済み) |
| identity handshake / `serverIdentity()` | ✅ (in-memory test 済み) |
| `StreamChannel.request` / `events` 実配線 | ✅ (request/response + event push、in-memory test 済み) |
| **NWProtocolQUIC stream adapter**（`NWMultiplexGroup`/`NWConnectionGroup`） | ✅ (実 quinn 相手の **live e2e** 済み) |
| `Endpoint.bonjour` discovery | ⬜ TODO |
| `DatagramChannel.events`(QUIC datagram demux) | ⬜ TODO |

> **テスト戦略**: channel 状態機械(open / request-response / event / identity)は `ChannelStream` 抽象 + in-memory paired stream で決定論的に検証(`ChannelTests`)。待ちの期限・cancel・後始末は `LifecycleTests`。実 NWProtocolQUIC stream はこの抽象に adapt 済み(`NWStreamChannel` / `QUICTransport`)。

### live e2e

`LiveE2ETests` は CI 非対象(環境変数で gate)。`unison mock`(quinn)相手に実 QUIC round-trip を確認する:

`Package.swift` は **repo root** にあり (SPM の version 指定 remote 依存が root manifest を
要求するため)。source 実体はこの `clients/swift/` 配下で、root manifest が `path:` で参照する。
よって `swift build` / `swift test` は **repo root** で実行する。

```bash
# 1) Rust 側で mock server を起動
target/debug/unison mock --schema schemas/ping_pong.kdl --addr '[::1]:7878'
# 2) Swift 側で live test (repo root で)
UNISON_LIVE=1 swift test --filter LiveE2E
```

connect(ALPN "unison")→ openChannel → request → response decode が **Apple NWProtocolQUIC ↔ Rust quinn** で interop することの最終証明。

`LifecycleLiveTests` も同じく環境変数で gate する。接続と切断を繰り返して process の UDP socket が増えないこと、server を止めたときに `.disconnected` が出ることを、実 QUIC で確かめる（server を止めるテストは自分で server を立てる。手順はファイル冒頭のコメント）:

```bash
cargo build -p unison-cli && cargo build -p club-unison --example handover_probe_server
target/debug/unison mock --schema schemas/ping_pong.kdl --addr '[::1]:17901' &
UNISON_LIFECYCLE_LIVE=1 \
UNISON_BIN=$PWD/target/debug/unison UNISON_SCHEMA=$PWD/schemas/ping_pong.kdl \
UNISON_PROBE_BIN=$PWD/target/debug/examples/handover_probe_server \
swift test --filter LifecycleLive
```

## 開発

### 回線切り替えの実機確認

QUIC の connection migration を `.handover` で opt-in する。Quinn サーバーとの
同じ接続・同じチャネルが Wi-Fi / 携帯回線をまたいで続くかは実機確認が必要。
[プローブと手順](../../guides/swift-handover.md) を参照。

```bash
# repo root で (Package.swift は root、source は clients/swift/ 配下)
swift build
swift test          # scaffold smoke test (型 + 純粋ロジック)
```

### wire 型の再生成

`protocol.proto` 変更時:

```bash
protoc \
  --proto_path=../../crates/unison-protocol/proto \
  --swift_out=Sources/UnisonClient/Wire/Generated \
  --swift_opt=Visibility=Public \
  ../../crates/unison-protocol/proto/protocol.proto
```

(`protoc-gen-swift` が必要: `brew install swift-protobuf`)

## 責務分割

- **club-unison(この package)**: generic client = transport / wire / framing / mux / handshake / channel 抽象 / conformance test。
- **consumer(例 VP)**: app 固有の `ChannelMeta`(KDL→Swift)/ UI / platform 統合。この package を SPM 依存にする。
