import Foundation
import Network
import Testing
@testable import UnisonClient

private struct HandoverMeta: StreamChannelMeta {
    static let name = "handover-probe"
    struct Event: Decodable, Sendable {}
}

private struct Probe: UnisonRequest {
    static let method = "Probe"
    let seq: Int
    struct Response: Decodable, Sendable {
        let seq: Int
        let received: Int
        let connection_id: String
        let channel_id: String
    }
}

/// Opt-in device probe. See guides/swift-handover.md for server and Xcode setup.
/// Never reconnects or reopens the channel; a successful run alone does not prove
/// a network handover occurred. Correlate it with PATH logs and the manual switch.
struct HandoverLiveTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["UNISON_HANDOVER"] == "1"))
    func sameConnectionAndChannelSurviveManualSwitch() async throws {
        let env = ProcessInfo.processInfo.environment
        let host = try #require(env["UNISON_HANDOVER_HOST"], "Set the reachable server host")
        let port = try #require(UInt16(env["UNISON_HANDOVER_PORT"] ?? "17878"))
        try #require(port > 0, "port must be 1...65535")
        let samples = try #require(Int(env["UNISON_HANDOVER_SAMPLES"] ?? "120"))
        try #require((2...300).contains(samples), "samples must be 2...300")
        let trust: TrustPolicy
        if let hex = env["UNISON_HANDOVER_CERT_HEX"] {
            try #require(!hex.isEmpty && hex.utf8.count.isMultiple(of: 2)
                         && hex.utf8.allSatisfy { (48...57).contains($0)
                             || (65...70).contains($0) || (97...102).contains($0) },
                         "Certificate pin must contain pairs of ASCII hex digits")
            var bytes = [UInt8]()
            var index = hex.startIndex
            while index < hex.endIndex {
                let end = hex.index(index, offsetBy: 2)
                bytes.append(try #require(UInt8(hex[index..<end], radix: 16)))
                index = end
            }
            trust = .pinned(Data(bytes))
        } else {
            // Only the loopback smoke test may bypass verification.
            try #require(["localhost", "127.0.0.1", "::1"].contains(host),
                         "Remote probes require UNISON_HANDOVER_CERT_HEX")
            trust = .skipVerify
        }

        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { path in
            let wifi = path.usesInterfaceType(.wifi)
            let cellular = path.usesInterfaceType(.cellular)
            let wired = path.usesInterfaceType(.wiredEthernet)
            let other = path.usesInterfaceType(.other)
            print("PATH time=\(Date().ISO8601Format()) status=\(path.status) wifi=\(wifi) cellular=\(cellular) wired=\(wired) other=\(other) expensive=\(path.isExpensive) interfaces=\(path.availableInterfaces) description=\(path)")
        }
        monitor.start(queue: DispatchQueue(label: "unison.handover-probe.path"))
        defer { monitor.cancel() }

        let conn = try await UnisonClient.connect(to: .host(host, port: port), trust: trust)
        do {
            let channel = try await conn.openChannel(HandoverMeta())
            // Group cancellation does not necessarily close existing streams.
            // Fail the test explicitly and close the channel to reject a pending
            // request; cancelling its Swift Task alone does not unblock it.
            let deadline = Task {
                do {
                    try await Task.sleep(for: .seconds(samples + 90))
                    Issue.record("DEADLINE: probe exceeded its request-loop time limit")
                    await channel.close()
                    await conn.disconnect()
                } catch { /* normal cancellation after the probe finishes */ }
            }
            defer { deadline.cancel() }
            let clock = ContinuousClock()
            var previousReply = clock.now
            var expectedConnection: String?
            var expectedChannel: String?
            for seq in 1...samples {
                let sent = clock.now
                let reply = try await channel.request(Probe(seq: seq))
                let received = clock.now
                try #require(reply.seq == seq)
                try #require(reply.received == seq, "Server-side channel counter reset")
                try #require(!reply.connection_id.isEmpty && !reply.channel_id.isEmpty)
                if let expectedConnection, let expectedChannel {
                    try #require(reply.connection_id == expectedConnection, "QUIC connection replaced")
                    try #require(reply.channel_id == expectedChannel, "Channel reopened")
                } else {
                    expectedConnection = reply.connection_id
                    expectedChannel = reply.channel_id
                }
                print("PROBE time=\(Date().ISO8601Format()) seq=\(seq) connection=\(reply.connection_id) channel=\(reply.channel_id) rtt=\(sent.duration(to: received)) gap=\(previousReply.duration(to: received))")
                previousReply = received
                if seq < samples { try await Task.sleep(for: .seconds(1)) }
            }
            await channel.close()
            await conn.disconnect()
            print("CONTINUITY_COMPLETE samples=\(samples); verify the manual network switch separately")
        } catch {
            await conn.disconnect()
            throw error
        }
    }
}
