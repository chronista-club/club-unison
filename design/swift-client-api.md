# design/swift-client-api.md — Swift Client SDK API Design

**バージョン**: 0.3（v1.3.0 で stream channel GA、v1.4.0 で auth、v2.2.0 で connection migration、v2.3.0 で待ちの期限・cancel と接続の後始末）
**最終更新**: 2026-11-17
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
| wire | `proto/protocol.proto` → `swift-protobuf` 生成（`Protocol_PacketHeader` / `Protocol_ProtocolMessage`）。コミット済みの生成物（`clients/swift/Sources/UnisonClient/Wire/Generated/protocol.pb.swift`）は v2.0 の reserved 化以前のままで、`PacketHeader` の field 6 / 8-11 を持つ（SDK は既定値のまま送るので wire 上の bytes は一致） |
| framing | typed frame `[4B BE len][1B type][UnisonPacket]`（Rust `network/frame.rs` と byte 一致、golden fixture 検証済み） |
| 圧縮 | 送信は非圧縮、受信は zstd 展開（libzstd）。展開後サイズは `Packet.maxPayloadSize` = 16MB で上限（`clients/swift/Sources/UnisonClient/Wire/Packet.swift`） |

## 3. API contract

```swift
public enum UnisonClient {
    public static func connect(to: Endpoint, trust: TrustPolicy) async throws -> Connection
}
public enum Endpoint    { case localDaemon(port: UInt16), host(String, port: UInt16), bonjour(String) }
public enum TrustPolicy { case system, skipVerify, pinned(Data) }

public actor Connection {
    public nonisolated var connectionEvents: AsyncStream<ConnectionEvent> { get }  // reconnect は caller 責務
    public func serverIdentity(timeout: Duration? = .seconds(5)) async throws -> ServerIdentity
    public func openChannel<M: StreamChannelMeta>(_ meta: M, timeout: Duration? = .seconds(5)) async throws -> StreamChannel<M>
    public func openDatagramChannel<M: DatagramChannelMeta>(_ meta: M) async throws -> DatagramChannel<M>
    public func disconnect() async
}
public struct StreamChannel<M: StreamChannelMeta>: Sendable {
    public var events: AsyncStream<M.Event> { get }
    public func request<R: UnisonRequest>(_ req: R, timeout: Duration? = .seconds(30)) async throws -> R.Response
    public func close() async
}
```

既定の期限は `UnisonClient.defaultRequestTimeout`（30 秒 = Rust `UnisonChannel` と同じ）/
`defaultOpenTimeout`（5 秒 = TS `openTimeoutMs` と同じ）/ `defaultIdentityTimeout`（5 秒）。
`nil` を渡すと期限を付けない。

idiom mapping: `AsyncIterable`→`AsyncStream`、`Promise`→`async throws`、生成
`ChannelMeta`→Swift generic + 生成型、actor で接続状態を隔離。

### 3.1 Auth API (connection-level、 v1.4.0)

connection-level auth primitive（Rust `enable_auth` / `connect_with_credential`、設計
`design/connection-auth.md`）の Swift 対応。`unison.auth` channel を open して `Authenticate`
を送るだけなので、専用 transport は不要。

```swift
public enum UnisonClient {
    // connect-time auth (= Rust connect_with_credential の対応物)。拒否なら throw。
    public static func connect(to: Endpoint, trust: TrustPolicy, credential: [UInt8]) async throws -> Connection
}
public actor Connection {
    // post-connect / re-auth 用。ok=false なら throw。
    public func authenticate(_ credential: [UInt8]) async throws
}
```

**wire 不変条件**（`design/connection-auth.md` §5.8 が SSOT）:
- request payload = `{ "credential": <number[]> }`。credential は **`[UInt8]` 配列で送る**。
  ⚠️ `Data` を `Codable` に入れると `JSONEncoder` が **base64 string** 化し Rust の `Vec<u8>`
  （数値配列）と非互換になるため、`[UInt8]` を使うこと。
- response = `{ "ok": Bool }`。`ok == false` → `authenticate` / `connect` が `UnisonError` を throw。

reserved channel meta / 定数（`AUTH_CHANNEL_NAME` = `"unison.auth"` / `AUTHENTICATE_METHOD` =
`"Authenticate"`）は SDK 内蔵、codegen 不要。

## 4. 設計判断

- **`request<R: M.Request>` → `request<R: UnisonRequest>`**: handoff sketch は
  associatedtype `M.Request` を generic constraint に使う形だったが、Swift は
  associatedtype を制約に使えないため `UnisonRequest` プロトコル制約へ置換。caller
  体験（`channel.request(SomeReq(...))`）は不変。
- **JSON codec 既定**: `UnisonRequest: Encodable` / `Response, Event: Decodable`。
  wire payload は TS と同じ既定 JSON codec。
- **reconnect は caller 責務**（library は auto-reconnect しない、TS と同方針）。
- **待ちは必ず戻る**（v2.3.0）: `connect` / `openChannel` / `request` / `serverIdentity` /
  `events` の待ちは、応答・期限（`UnisonError.timeout`。`connect` は 10 秒固定）・Task の
  cancel（`CancellationError`）・接続や stream の終わり（`UnisonError.notConnected` ほか）の
  どれかで戻る。`serverIdentity` の読み込みは接続で 1 本にまとめ、同時に呼んだ全員が
  同じ値を受け取る。期限や cancel で戻ったあとに届いた応答は捨てる。受信が終わった
  channel（server が stream を閉じた / 壊れた frame を受けた）への request は、
  待たずに失敗する。
- **接続の終わりは `connectionEvents` に出る**（v2.3.0）: `disconnect()` だけでなく、
  server が接続を閉じた場合や経路が失敗した場合にも `.disconnected(reason:)` を 1 回流す。
  検知の速さは相手の終わり方で決まる。server が接続を閉じてから終わった場合
  （`ServerHandle::shutdown()`）はすぐ、黙って消えた場合（crash / kill）は QUIC の
  idle timeout（最大 30 秒）で検知する（実測、`LifecycleLiveTests`）。もっと早く知りたい
  app は心拍を併用する。
- **資源の後始末**（v2.3.0）: Network.framework は、cancel されない `NWConnection`
  （stream）が 1 本でも残ると QUIC 接続と UDP socket を生かし続ける。`disconnect()` は
  開いた stream と、受け取り手のいない server 起点の stream（identity）を全部閉じる。
  `disconnect()` / `close()` を呼ばずに `Connection` / `StreamChannel` を手放した場合も、
  deinit で閉じる。open に失敗した stream、読み終えた identity stream も閉じる。
- **event は 256 件まで溜める**（v2.3.0）: 読まれていない event の上限は Rust
  `UnisonChannel` の受信 queue と同じ 256 件で、超えて届いた分は捨てる。`events` は
  途中で読むのをやめても channel を終わらせず、もう一度回せば続きから読める。
- **connection migration**: `NWParameters.multipathServiceType = .handover` を既定で
  設定し、OS に回線変更時の接続継続を依頼する。再接続や QUIC Multipath 拡張の
  導入ではない。圏外や idle timeout を越えた停止からの復旧は引き続き caller 責務。
  検証は [Swift handover guide](../guides/swift-handover.md) の同一接続・チャネル
  ID と連番で判定し、実機で未観測の handover をローカル試験から推定しない。
- **transport 抽象**: `ChannelStream` / `ChannelTransport` を挟み、channel 状態機械
  （open / request-response / event / identity）を NWProtocolQUIC から分離。in-memory
  paired stream で決定論的にテストし、実 stream は薄い adapter で接続（= TS の
  `MockTransport` / `StreamServerStub` と同じテスト構造）。

## 5. codegen 2 層

1. **wire**: `protocol.proto` → swift-protobuf（生成物はコミット、consumer は protoc 不要）。
   コミット済みの生成物は v2.0 の reserved 化以前のまま（§2）
2. **app**: KDL schema → Swift channel meta + 型（当面手書き、将来 KDL→Swift codegen）。
   consumer の `VPProtocol`（`vp-world.kdl`→Swift）は VP repo 側に置く。

## 6. 実装状況 / 残り

GA: stream channel（connect / openChannel / request-response / event / identity）+
connection-level auth（`connect(...credential:)` / `Connection.authenticate`、v1.4.0）。
`unison mock`（quinn）相手の live e2e PASS。待ちの期限・cancel と接続の後始末は
in-memory（`LifecycleTests`）と実 QUIC（`LifecycleLiveTests`: UDP socket の数、
server を止めたときの `.disconnected`）の両方で確認する。

残り（後続 pass）: `DatagramChannel`（QUIC datagram demux）/ `Endpoint.bonjour`
discovery / KDL→Swift channel-meta codegen。

> 実装詳細・ビルド・live e2e 手順は [`clients/swift/README.md`](../clients/swift/README.md)。
