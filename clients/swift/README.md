# UnisonClient (Swift)

Unison protocol の **Swift client SDK**。`clients/ruby` / `clients/typescript` の swift sibling で、wire-format conformance / protocol semantics / version 互換は club-unison framework 側が owns する。consumer (例: Vantage Point の macOS menu bar agent / visionOS app) はこの package を SPM 依存として使う。

> **Status**: `scaffold` — package 構造 / 公開 API contract / wire 型 / NWProtocolQUIC transport (handshake) まで。framing / channel mux / identity handshake は後続 pass。下記「実装状況」参照。

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

// server push を観測 (reconnect は caller 責務)
for await event in conn.connectionEvents { /* .connected / .disconnected */ }

let channel = try await conn.openChannel(SomeStreamChannelMeta())
let response = try await channel.request(SomeRequest(...))   // async throws
for await event in channel.events { /* server → client push */ }
await channel.close()

await conn.disconnect()
```

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
| **NWProtocolQUIC stream adapter**（`QUICTransport.openStream`/`acceptStream`） | ⬜ TODO (= 抽象を実 QUIC stream に接続、live e2e) |
| `Endpoint.bonjour` discovery | ⬜ TODO |
| `DatagramChannel.events`(QUIC datagram demux) | ⬜ TODO |

> **テスト戦略**: channel 状態機械(open / request-response / event / identity)は `ChannelStream` 抽象 + in-memory paired stream で決定論的に検証(`ChannelTests`)。NWProtocolQUIC の実 stream は次 pass でこの抽象に adapt し、`unison mock` 相手の live e2e を通す。

## 開発

```bash
cd clients/swift
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
