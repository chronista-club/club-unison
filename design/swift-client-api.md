# design/swift-client-api.md — Swift Client SDK API Design

**バージョン**: 0.1（v1.3.0 で stream channel GA）
**最終更新**: 2026-06-25
**ステータス**: Implemented（`clients/swift`）— stream channel が実 QUIC で動作

---

## 1. 目的

`clients/{ruby,typescript}` の swift sibling として、Apple platform 面（macOS
menu bar agent / visionOS app）が Unison server に **pure Apple-native Swift** で
接続するための generic client SDK。VP 非依存。wire-format conformance / protocol
semantics / version 互換は本 framework が owns し、consumer（VP の `VPProtocol`
等）は app 固有 channel meta を載せるだけにする。

設計は `design/typescript-client-api.md` の ideal-caller-first を踏襲（同形式・同体験）。

## 2. transport / wire

| 層 | 実装 |
|----|------|
| transport | Apple `Network.framework` の `NWProtocolQUIC`（生 QUIC: streams + datagrams, TLS1.3） |
| stream 多重化 | `NWMultiplexGroup` + `NWConnectionGroup`（client 起点 = `NWConnection(from:)`、server 起点 = `newConnectionHandler`） |
| ALPN | `"unison"` 固定 = Rust `network::UNISON_ALPN`（QUIC は RFC 9001 §8.1 で ALPN 必須、Apple は強制） |
| wire | `proto/protocol.proto` → `swift-protobuf` 生成（`Protocol_PacketHeader` / `Protocol_ProtocolMessage`） |
| framing | typed frame `[4B BE len][1B type][UnisonPacket]`（Rust `quic.rs` と byte 一致、golden fixture 検証済み） |

## 3. API contract

```swift
public enum UnisonClient {
    public static func connect(to: Endpoint, trust: TrustPolicy) async throws -> Connection
}
public enum Endpoint    { case localDaemon(port: UInt16), host(String, port: UInt16), bonjour(String) }
public enum TrustPolicy { case system, skipVerify, pinned(Data) }

public actor Connection {
    public nonisolated var connectionEvents: AsyncStream<ConnectionEvent> { get }  // reconnect は caller 責務
    public func serverIdentity() async throws -> ServerIdentity
    public func openChannel<M: StreamChannelMeta>(_ meta: M) async throws -> StreamChannel<M>
    public func openDatagramChannel<M: DatagramChannelMeta>(_ meta: M) async throws -> DatagramChannel<M>
    public func disconnect() async
}
public struct StreamChannel<M: StreamChannelMeta>: Sendable {
    public var events: AsyncStream<M.Event> { get }
    public func request<R: UnisonRequest>(_ req: R) async throws -> R.Response
    public func close() async
}
```

idiom mapping: `AsyncIterable`→`AsyncStream`、`Promise`→`async throws`、生成
`ChannelMeta`→Swift generic + 生成型、actor で接続状態を隔離。

## 4. 設計判断

- **`request<R: M.Request>` → `request<R: UnisonRequest>`**: handoff sketch は
  associatedtype `M.Request` を generic constraint に使う形だったが、Swift は
  associatedtype を制約に使えないため `UnisonRequest` プロトコル制約へ置換。caller
  体験（`channel.request(SomeReq(...))`）は不変。
- **JSON codec 既定**: `UnisonRequest: Encodable` / `Response, Event: Decodable`。
  wire payload は TS と同じ既定 JSON codec。
- **reconnect は caller 責務**（library は auto-reconnect しない、TS と同方針）。
- **transport 抽象**: `ChannelStream` / `ChannelTransport` を挟み、channel 状態機械
  （open / request-response / event / identity）を NWProtocolQUIC から分離。in-memory
  paired stream で決定論的にテストし、実 stream は薄い adapter で接続（= TS の
  `MockTransport` / `StreamServerStub` と同じテスト構造）。

## 5. codegen 2 層

1. **wire**: `protocol.proto` → swift-protobuf（生成物はコミット、consumer は protoc 不要）
2. **app**: KDL schema → Swift channel meta + 型（当面手書き、将来 KDL→Swift codegen）。
   consumer の `VPProtocol`（`vp-world.kdl`→Swift）は VP repo 側に置く。

## 6. 実装状況 / 残り

GA: stream channel（connect / openChannel / request-response / event / identity）。
`unison mock`（quinn）相手の live e2e PASS。

残り（後続 pass）: `DatagramChannel`（QUIC datagram demux）/ `Endpoint.bonjour`
discovery / KDL→Swift channel-meta codegen。

> 実装詳細・ビルド・live e2e 手順は [`clients/swift/README.md`](../clients/swift/README.md)。
