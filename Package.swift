// swift-tools-version: 6.0
import PackageDescription

// Unison protocol — Swift client SDK (polyglot client base、 server stays Rust)。
//
// この Package.swift は **monorepo root** に置く。SPM は version 指定リモート依存
// (`.package(url:, from:)`) に「repo root の manifest」を要求し、 subdirectory の
// manifest を解決できないため。実体 source は `clients/swift/` 配下に集約したまま、
// target の `path:` で参照する (= monorepo にコードを集約しつつ SPM 配布も成立)。
//
// 版数は monorepo の git tag (`vX.Y.Z` = Rust workspace 版) に連動する (= 意図的。
// club-unison 全体で揃った版数で配布、 Swift client を独立 versioning しない)。
// consumer: `.package(url: "https://github.com/chronista-club/club-unison.git", from: "1.4.0")`
//
// transport = Apple Network.framework の NWProtocolQUIC (生 QUIC: streams +
// datagrams, ALPN "unison")、 wire = swift-protobuf。
let package = Package(
    name: "UnisonClient",
    platforms: [
        .macOS(.v13),
        .iOS(.v16),
        .visionOS(.v1),
    ],
    products: [
        .library(name: "UnisonClient", targets: ["UnisonClient"]),
    ],
    dependencies: [
        // wire format = protocol.proto → swift-protobuf 生成 (Apple 公式)。
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.38.0"),
    ],
    targets: [
        .target(
            name: "UnisonClient",
            dependencies: [
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            // source 実体は monorepo の clients/swift/ 配下 (root manifest から path 参照)。
            path: "clients/swift/Sources/UnisonClient"
        ),
        .testTarget(
            name: "UnisonClientTests",
            dependencies: ["UnisonClient"],
            path: "clients/swift/Tests/UnisonClientTests",
            // Rust `tests/fixtures/wire/` の golden byte vector を取り込み、
            // Swift encoder の出力が Rust と byte 一致することを検証する。
            resources: [.copy("Fixtures")]
        ),
    ]
)
