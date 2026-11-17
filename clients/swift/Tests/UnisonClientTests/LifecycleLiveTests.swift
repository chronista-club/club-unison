import Foundation
import Testing
@testable import UnisonClient

// live: 実 NWProtocolQUIC ↔ quinn で、 接続の後始末と、 相手が居なくなったときの通知を確かめる
// (= in-memory では見えない Network.framework 側の振る舞い)。
//
//   1) cargo build -p unison-cli && cargo build -p club-unison --example handover_probe_server
//   2) target/debug/unison mock --schema schemas/ping_pong.kdl --addr '[::1]:17901'
//   3) 次の環境変数を付けて `swift test --filter LifecycleLive`:
//        UNISON_LIFECYCLE_LIVE=1
//        UNISON_BIN=$PWD/target/debug/unison
//        UNISON_SCHEMA=$PWD/schemas/ping_pong.kdl
//        UNISON_PROBE_BIN=$PWD/target/debug/examples/handover_probe_server
//
// server を止めるテストは、 自分で別の port (17902) に server を立てる:
// - `unison mock` (UNISON_BIN / UNISON_SCHEMA) — 終了処理をしない。 止めると黙って消える
// - `handover_probe_server` (UNISON_PROBE_BIN) — Ctrl-C で `ServerHandle::shutdown()` を呼ぶ

/// この process が持つ UDP socket の数 (`lsof -a -p <pid> -iUDP`)。
private func udpSocketCount() -> Int {
    let lsof = Process()
    lsof.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
    lsof.arguments = ["-a", "-p", String(ProcessInfo.processInfo.processIdentifier), "-iUDP", "-Fn"]
    let out = Pipe()
    lsof.standardOutput = out
    lsof.standardError = Pipe()
    try? lsof.run()
    lsof.waitUntilExit()
    let text = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    return text.split(separator: "\n").filter { $0.hasPrefix("n") }.count
}

private struct PingPongMeta: StreamChannelMeta {
    static let name = "ping-pong"
    struct Tick: Codable, Sendable {}
    typealias Event = Tick
}

/// 繰り返すたびに socket が増えていれば漏れ。 基準値との差は 0 以下なら可とする
/// (= 並行して走る別の live テストの socket が、 基準値に混ざることがある)。
private func leaked(_ counts: [Int]) -> Bool {
    (counts.last ?? 0) > 0
}

/// `unison mock` を子 process で立てる。
private func startMockServer(port: UInt16) async throws -> Process {
    let env = ProcessInfo.processInfo.environment
    guard let bin = env["UNISON_BIN"], let schema = env["UNISON_SCHEMA"] else {
        throw UnisonError.transport("UNISON_BIN / UNISON_SCHEMA が未設定")
    }
    return try await startServer(bin: bin, arguments: ["mock", "--schema", schema, "--addr", "[::1]:\(port)"], port: port)
}

/// `handover_probe_server` (= Ctrl-C で graceful shutdown する server) を子 process で立てる。
private func startGracefulServer(port: UInt16) async throws -> Process {
    guard let bin = ProcessInfo.processInfo.environment["UNISON_PROBE_BIN"] else {
        throw UnisonError.transport("UNISON_PROBE_BIN が未設定")
    }
    return try await startServer(bin: bin, arguments: ["[::1]:\(port)"], port: port)
}

/// server を子 process で立て、 つながるようになるまで待つ。
private func startServer(bin: String, arguments: [String], port: UInt16) async throws -> Process {
    let server = Process()
    server.executableURL = URL(fileURLWithPath: bin)
    server.arguments = arguments
    let out = Pipe()
    server.standardOutput = out
    server.standardError = out
    try server.run()
    // 起動を待つ: つながるまで試す。
    for _ in 0..<50 {
        if let conn = try? await UnisonClient.connect(to: .localDaemon(port: port), trust: .skipVerify) {
            await conn.disconnect()
            return server
        }
        try await Task.sleep(for: .milliseconds(100))
    }
    server.terminate()
    throw UnisonError.transport("server が起動しない: \(bin)")
}

/// `connectionEvents` から `.disconnected` が出るまでの秒数。 `limit` 秒で出なければ nil。
private func secondsUntilDisconnected(_ conn: Connection, limit: Double) async -> (Double, String?)? {
    let start = Date()
    return try? await finishes(within: limit) {
        for await event in conn.connectionEvents {
            if case .disconnected(let reason) = event {
                return (Date().timeIntervalSince(start), reason)
            }
        }
        throw UnisonError.notConnected
    }
}

@Suite(.serialized)
struct LifecycleLiveTests {
    private static var enabled: Bool {
        ProcessInfo.processInfo.environment["UNISON_LIFECYCLE_LIVE"] == "1"
    }

    private static var port: UInt16 {
        UInt16(ProcessInfo.processInfo.environment["UNISON_LIFECYCLE_PORT"] ?? "") ?? 17901
    }

    /// 接続して切断するたびに UDP socket が 1 本ずつ残らないこと
    /// (lnx creare で一晩に 1,497 本溜まった件の回帰)。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func disconnectReleasesUDPSocket() async throws {
        let before = udpSocketCount()
        var counts: [Int] = []
        for _ in 0..<5 {
            let conn = try await UnisonClient.connect(to: .localDaemon(port: Self.port), trust: .skipVerify)
            await conn.disconnect()
            try await Task.sleep(for: .milliseconds(300))
            counts.append(udpSocketCount() - before)
        }
        print("LIFECYCLE udp sockets after each disconnect: \(counts)")
        #expect(leaked(counts) == false, "disconnect 後に UDP socket が残っている: \(counts)")
    }

    /// channel を閉じ忘れたまま disconnect しても socket が残らないこと。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func disconnectWithOpenChannelReleasesUDPSocket() async throws {
        let before = udpSocketCount()
        var counts: [Int] = []
        for _ in 0..<5 {
            let conn = try await UnisonClient.connect(to: .localDaemon(port: Self.port), trust: .skipVerify)
            _ = try await conn.openChannel(PingPongMeta())
            await conn.disconnect()
            try await Task.sleep(for: .milliseconds(300))
            counts.append(udpSocketCount() - before)
        }
        print("LIFECYCLE udp sockets (open channel left): \(counts)")
        #expect(leaked(counts) == false, "channel を残した disconnect 後に UDP socket が残っている: \(counts)")
    }

    /// disconnect() を呼ばずに Connection を手放しても socket が残らないこと。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func droppedConnectionReleasesUDPSocket() async throws {
        let before = udpSocketCount()
        var counts: [Int] = []
        for _ in 0..<5 {
            do {
                let conn = try await UnisonClient.connect(to: .localDaemon(port: Self.port), trust: .skipVerify)
                _ = try await conn.openChannel(PingPongMeta())
            }
            try await Task.sleep(for: .milliseconds(300))
            counts.append(udpSocketCount() - before)
        }
        print("LIFECYCLE udp sockets (connection dropped): \(counts)")
        #expect(leaked(counts) == false, "手放した Connection の UDP socket が残っている: \(counts)")
    }

    /// identity を読んだあとの disconnect でも socket が残らないこと。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func disconnectAfterServerIdentityReleasesUDPSocket() async throws {
        let before = udpSocketCount()
        var counts: [Int] = []
        for _ in 0..<5 {
            let conn = try await UnisonClient.connect(to: .localDaemon(port: Self.port), trust: .skipVerify)
            _ = try await conn.serverIdentity()
            await conn.disconnect()
            try await Task.sleep(for: .milliseconds(300))
            counts.append(udpSocketCount() - before)
        }
        print("LIFECYCLE udp sockets (after serverIdentity): \(counts)")
        #expect(leaked(counts) == false, "identity を読んだ disconnect 後に UDP socket が残っている: \(counts)")
    }

    /// server が居ない port への接続待ちは、 Task の cancel で戻り、 socket を残さない
    /// (= 10 秒の接続期限を待たない)。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func connectReturnsOnCancelAndReleasesUDPSocket() async throws {
        let before = udpSocketCount()
        let task = Task { try await UnisonClient.connect(to: .localDaemon(port: 17999), trust: .skipVerify) }
        try await Task.sleep(for: .milliseconds(300))
        let start = Date()
        task.cancel()
        await #expect(throws: CancellationError.self) {
            _ = try await finishes(within: 3) { try await task.value }
        }
        print("LIFECYCLE connect cancel returned in \(Date().timeIntervalSince(start))s")
        try await Task.sleep(for: .milliseconds(300))
        #expect(udpSocketCount() - before <= 0)
    }

    /// server が終了 (SIGTERM) したら `.disconnected` が出て、 待っている event の読み手も戻る。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func serverTerminationEmitsDisconnected() async throws {
        let before = udpSocketCount()
        let server = try await startMockServer(port: 17902)
        let conn = try await UnisonClient.connect(to: .localDaemon(port: 17902), trust: .skipVerify)
        let channel = try await conn.openChannel(PingPongMeta())

        server.terminate()
        let result = await secondsUntilDisconnected(conn, limit: 90)
        print("LIFECYCLE SIGTERM → disconnected: \(String(describing: result))")
        #expect(result != nil, "server の終了後 90 秒待っても .disconnected が出ない")
        // 切れたあとの channel は、 待たずに終わる。
        let ended = try await finishes(within: 3) {
            for await _ in channel.events {}
            return true
        }
        #expect(ended)
        try await Task.sleep(for: .milliseconds(300))
        #expect(udpSocketCount() - before <= 0)
    }

    /// server が graceful shutdown (= 接続を閉じてから終了) した場合は、 すぐ `.disconnected` が出る。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func gracefulServerShutdownEmitsDisconnectedPromptly() async throws {
        let before = udpSocketCount()
        let server = try await startGracefulServer(port: 17902)
        let conn = try await UnisonClient.connect(to: .localDaemon(port: 17902), trust: .skipVerify)

        kill(server.processIdentifier, SIGINT)
        let result = await secondsUntilDisconnected(conn, limit: 90)
        print("LIFECYCLE graceful shutdown → disconnected: \(String(describing: result))")
        #expect(result != nil, "server の graceful shutdown 後 90 秒待っても .disconnected が出ない")
        if let result {
            #expect(result.0 < 5, "graceful shutdown は idle timeout を待たずに検知できるはず")
        }
        try await Task.sleep(for: .milliseconds(300))
        #expect(udpSocketCount() - before <= 0)
    }

    /// server が突然死 (SIGKILL) した場合も `.disconnected` が出る。
    @Test(.enabled(if: LifecycleLiveTests.enabled))
    func serverKillEmitsDisconnected() async throws {
        let before = udpSocketCount()
        let server = try await startMockServer(port: 17902)
        let conn = try await UnisonClient.connect(to: .localDaemon(port: 17902), trust: .skipVerify)

        kill(server.processIdentifier, SIGKILL)
        let result = await secondsUntilDisconnected(conn, limit: 90)
        print("LIFECYCLE SIGKILL → disconnected: \(String(describing: result))")
        #expect(result != nil, "server の突然死後 90 秒待っても .disconnected が出ない")
        try await Task.sleep(for: .milliseconds(300))
        #expect(udpSocketCount() - before <= 0)
    }
}
