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
        // zstd 展開 (facebook 公式が SPM 対応)。 Rust server は 2KB 以上の payload を
        // 自動で zstd 圧縮する (packet/mod.rs) — 受信側の展開が無いと大きな
        // response / event が読めない (実測 2026-08-15: fieldd の 64-entity 応答)。
        // Apple Compression framework は zstd 非対応のため公式 C 実装を使う。
        .package(url: "https://github.com/facebook/zstd.git", from: "1.5.7"),
    ],
    targets: [
        .target(
            name: "UnisonClient",
            dependencies: [
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
                .product(name: "libzstd", package: "zstd"),
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
