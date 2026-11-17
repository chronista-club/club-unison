import Foundation
import Testing
@testable import UnisonClient

// channel / connection の後始末と、 待ちが必ず戻ることの in-memory テスト。
// 「server が固まる・勝手に閉じる・event を送り続ける」と、 「app が cancel する・
// 購読をやめる・閉じ忘れる」の組み合わせで、 永久に待つ経路と資源の残りが無いことを固める。

private struct PingPongMeta: StreamChannelMeta {
    static let name = "ping-pong"
    struct Tick: Codable, Sendable, Equatable { let seq: Int }
    typealias Event = Tick
}

private struct EchoReq: UnisonRequest {
    static let method = "Echo"
    struct Resp: Codable, Sendable, Equatable { let data: String }
    typealias Response = Resp
    let data: String
}

/// stub が echo で応える、 もう 1 つの method。
private struct OtherReq: UnisonRequest {
    static let method = "Other"
    typealias Response = EchoReq.Resp
    let data: String
}

private func ticks(_ count: Int) -> [Data] {
    (0..<count).map { Data(#"{"seq":\#($0)}"#.utf8) }
}

struct LifecycleTests {
    // MARK: - 待ちが必ず戻る

    /// server が stream を閉じたあとの request は、 待たずに失敗する。
    @Test func requestAfterServerClosedStreamFails() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())
        await transport.serverStreams[0].close()

        await #expect(throws: UnisonError.self) {
            try await finishes { try await channel.request(EchoReq(data: "x")) }
        }
    }

    /// 壊れた frame で受信が止まったあとの request も、 待たずに失敗する。
    /// 受信をやめた stream は閉じる。
    @Test func requestAfterBrokenFrameFailsAndClosesStream() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())
        // 未知の type tag 0x7f を持つ typed frame
        try await transport.serverStreams[0].send(Data([0, 0, 0, 2, 0x7f, 0x00]))

        await #expect(throws: UnisonError.self) {
            try await finishes { try await channel.request(EchoReq(data: "x")) }
        }
        #expect(await eventually { transport.clientStreams[0].closeCount > 0 })
    }

    /// 応答を返さない server への request は、 Task の cancel で戻る。
    @Test func requestReturnsOnCancel() async throws {
        let transport = ControlledTransport(silent: ["Echo"])
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())

        let task = Task { try await channel.request(EchoReq(data: "x")) }
        try await Task.sleep(for: .milliseconds(50))
        task.cancel()

        await #expect(throws: CancellationError.self) {
            try await finishes { try await task.value }
        }
    }

    /// 応答を返さない server への request は、 期限で `timeout` になる。
    @Test func requestTimesOut() async throws {
        let transport = ControlledTransport(silent: ["Echo"])
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())

        do {
            _ = try await finishes { try await channel.request(EchoReq(data: "x"), timeout: .milliseconds(100)) }
            Issue.record("期限を過ぎても戻った")
        } catch UnisonError.timeout {
            // 期待どおり
        }
        // 期限切れのあとも、 同じ channel で別の request は通る。
        let resp = try await finishes { try await channel.request(OtherReq(data: "ok")) }
        #expect(resp.data == "ok")
    }

    /// open に応えない server は、 期限で `timeout` になり、 開きかけた stream を閉じる。
    @Test func openChannelTimesOutAndClosesStream() async throws {
        let transport = ControlledTransport(silent: ["__channel:ping-pong"])
        let conn = Connection(transport: transport)

        do {
            _ = try await finishes { try await conn.openChannel(PingPongMeta(), timeout: .milliseconds(100)) }
            Issue.record("期限を過ぎても戻った")
        } catch UnisonError.timeout {
            // 期待どおり
        }
        #expect(await eventually { transport.clientStreams[0].closeCount > 0 })
    }

    /// open の待ちは Task の cancel で戻り、 開きかけた stream を閉じる。
    @Test func openChannelReturnsOnCancel() async throws {
        let transport = ControlledTransport(silent: ["__channel:ping-pong"])
        let conn = Connection(transport: transport)

        let task = Task { try await conn.openChannel(PingPongMeta(), timeout: nil) }
        try await Task.sleep(for: .milliseconds(50))
        task.cancel()

        await #expect(throws: CancellationError.self) {
            _ = try await finishes { try await task.value }
        }
        #expect(await eventually { transport.clientStreams[0].closeCount > 0 })
    }

    /// identity を送らない server に対して、 serverIdentity は期限で `timeout` になる。
    @Test func serverIdentityTimesOut() async throws {
        let conn = Connection(transport: ControlledTransport())
        do {
            _ = try await finishes { try await conn.serverIdentity(timeout: .milliseconds(100)) }
            Issue.record("期限を過ぎても戻った")
        } catch UnisonError.timeout {
            // 期待どおり
        }
    }

    /// identity は読み終えたら stream を閉じ、 2 回目以降は同じ値を返す
    /// (= server は identity を 1 回しか送らない)。
    @Test func serverIdentityClosesStreamAndIsCached() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        let stream = await transport.pushIdentity()

        let first = try await finishes { try await conn.serverIdentity() }
        #expect(first.name == "stub")
        #expect(await eventually { stream.closeCount > 0 })
        let second = try await finishes { try await conn.serverIdentity() }
        #expect(second == first)
    }

    /// serverIdentity の待ちは、 期限の有無によらず cancel で `CancellationError` として戻る。
    @Test(arguments: [nil, Duration.seconds(5)])
    func serverIdentityReturnsOnCancel(timeout: Duration?) async throws {
        for _ in 0..<20 {
            let conn = Connection(transport: ControlledTransport())
            let task = Task { try await conn.serverIdentity(timeout: timeout) }
            try await Task.sleep(for: .milliseconds(10))
            task.cancel()
            await #expect(throws: CancellationError.self) {
                _ = try await finishes { try await task.value }
            }
        }
    }

    /// serverIdentity を同時に呼んでも、 1 本の identity stream を両方が受け取る。
    @Test func concurrentServerIdentityCallsShareOneRead() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        let a = Task { try await conn.serverIdentity() }
        let b = Task { try await conn.serverIdentity() }
        try await Task.sleep(for: .milliseconds(50))
        await transport.pushIdentity()

        let names = try await finishes { [try await a.value.name, try await b.value.name] }
        #expect(names == ["stub", "stub"])
    }

    /// 期限までに identity が届かなくても、 あとから届けば次の呼び出しで読める。
    @Test func serverIdentitySucceedsAfterEarlierTimeout() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        do {
            _ = try await finishes { try await conn.serverIdentity(timeout: .milliseconds(50)) }
            Issue.record("identity が無いのに戻った")
        } catch UnisonError.timeout {
            // 期待どおり
        }
        await transport.pushIdentity()
        let identity = try await finishes { try await conn.serverIdentity() }
        #expect(identity.name == "stub")
    }

    // MARK: - 接続の終わり

    /// 読まれなかった identity stream は、 disconnect で閉じる
    /// (= 閉じない stream は QUIC 接続と UDP socket を生かし続ける)。
    @Test func disconnectClosesUnreadIdentityStream() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        let stream = await transport.pushIdentity()
        #expect(stream.closeCount == 0)

        await conn.disconnect()
        #expect(await eventually { stream.closeCount > 0 })
    }

    /// 相手からの切断は `.disconnected(reason:)` として 1 回流れ、 transport を片付ける。
    @Test func remoteCloseEmitsDisconnected() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        await transport.simulateRemoteClose(reason: "peer gone")
        // 1 回目が処理されてから、 余分な通知を送る (= 2 回目は無視される)。
        #expect(await eventually { transport.closeCount == 1 })
        await transport.simulateRemoteClose(reason: "again")

        let events = try await finishes {
            var seen: [ConnectionEvent] = []
            for await event in conn.connectionEvents { seen.append(event) }
            return seen
        }
        #expect(events == [.connected, .disconnected(reason: "peer gone")])
        #expect(transport.closeCount == 1)
        // 切れた接続では channel を開けない。
        await #expect(throws: UnisonError.self) {
            _ = try await finishes { try await conn.openChannel(PingPongMeta()) }
        }
    }

    /// disconnect() は何度呼んでもよく、 `.disconnected(reason: nil)` は 1 回だけ流れる。
    /// 開いていた channel の待ちは戻る。
    @Test func disconnectIsIdempotentAndReleasesWaiters() async throws {
        let transport = ControlledTransport(silent: ["Echo"])
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())
        let waiting = Task { try await channel.request(EchoReq(data: "x"), timeout: nil) }
        try await Task.sleep(for: .milliseconds(50))

        await conn.disconnect()
        await conn.disconnect()

        await #expect(throws: UnisonError.self) {
            _ = try await finishes { try await waiting.value }
        }
        let events = try await finishes {
            var seen: [ConnectionEvent] = []
            for await event in conn.connectionEvents { seen.append(event) }
            return seen
        }
        #expect(events == [.connected, .disconnected(reason: nil)])
    }

    // MARK: - event

    /// 読まれない event は 256 件までしか溜めない (= Rust の受信 queue と同じ上限)。
    @Test func unreadEventsAreBounded() async throws {
        let transport = ControlledTransport(pushEvents: ticks(1000))
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())
        // stub は open の処理の中で event を全部送ってから次の request を読む。 同じ stream は
        // 順序を保つので、 この応答が返った時点で 1000 件とも client に届いている。
        _ = try await channel.request(EchoReq(data: "sync"))
        await transport.serverStreams[0].close()

        let received = try await finishes {
            var seqs: [Int] = []
            for await event in channel.events { seqs.append(event.seq) }
            return seqs
        }
        #expect(received.count == 256)
        #expect(received.first == 0)
    }

    /// event の購読を途中でやめても、 次の購読で続きが読める。
    @Test func eventsCanBeResubscribedAfterBreak() async throws {
        let transport = ControlledTransport(pushEvents: ticks(3))
        let conn = Connection(transport: transport)
        let channel = try await conn.openChannel(PingPongMeta())

        var first: Int?
        for await event in channel.events {
            first = event.seq
            break
        }
        let second = try await finishes { () -> Int? in
            for await event in channel.events { return event.seq }
            return nil
        }
        #expect(first == 0)
        #expect(second == 1)
        // 購読をやめても request は使える
        let resp = try await finishes { try await channel.request(EchoReq(data: "still")) }
        #expect(resp.data == "still")
    }

    // MARK: - 後始末

    /// open が拒否されたら、 開いた stream を閉じる。
    @Test func rejectedOpenClosesStream() async throws {
        let transport = ControlledTransport(rejectOpen: true)
        let conn = Connection(transport: transport)
        await #expect(throws: UnisonError.self) {
            _ = try await conn.openChannel(PingPongMeta())
        }
        #expect(await eventually { transport.clientStreams[0].closeCount > 0 })
    }

    /// close() を呼ばずに channel を手放しても、 stream は閉じる。
    @Test func droppedChannelClosesStream() async throws {
        let transport = ControlledTransport()
        let conn = Connection(transport: transport)
        do {
            _ = try await conn.openChannel(PingPongMeta())
        }
        #expect(await eventually { transport.clientStreams[0].closeCount > 0 })
    }

    /// accept 待ちが 2 つあっても、 両方に stream が渡る。
    @Test func pendingStreamsServesEveryWaiter() async throws {
        let pending = PendingStreams()
        let a = Task { await pending.pop() }
        let b = Task { await pending.pop() }
        try await Task.sleep(for: .milliseconds(50))
        await pending.push(InMemoryPair.make().client)
        await pending.push(InMemoryPair.make().client)

        let got = try await finishes { [await a.value != nil, await b.value != nil] }
        #expect(got == [true, true])
    }

    /// 受け取り手のいない stream は 16 本まで溜め、 それを超えた分は閉じる。
    @Test func pendingStreamsClosesStreamsOverTheLimit() async throws {
        let pending = PendingStreams()
        var streams: [InMemoryStream] = []
        for _ in 0..<17 {
            let stream = InMemoryPair.make().client
            streams.append(stream)
            await pending.push(stream)
        }
        #expect(streams.prefix(16).allSatisfy { $0.closeCount == 0 })
        #expect(streams[16].closeCount == 1)
    }

    /// finish すると、 残っていた stream を閉じ、 以降に届いた stream も閉じる。
    @Test func pendingStreamsFinishClosesLeftoversAndLateArrivals() async throws {
        let pending = PendingStreams()
        let leftover = InMemoryPair.make().client
        await pending.push(leftover)
        await pending.finish()
        #expect(leftover.closeCount == 1)

        let late = InMemoryPair.make().client
        await pending.push(late)
        #expect(late.closeCount == 1)
        #expect(await pending.pop() == nil)
    }

    /// accept 待ちは Task の cancel で戻る。
    @Test func pendingStreamsPopReturnsOnCancel() async throws {
        let pending = PendingStreams()
        let task = Task { await pending.pop() }
        try await Task.sleep(for: .milliseconds(50))
        task.cancel()
        let got = try await finishes { await task.value }
        #expect(got == nil)
    }
}
