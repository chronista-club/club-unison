// swift-tools-version: 6.0
import PackageDescription

// Unison protocol — Swift client SDK (polyglot client base、 server stays Rust)。
// ruby / typescript の swift sibling。 transport = Apple Network.framework の
// NWProtocolQUIC (生 QUIC: streams + datagrams, ALPN "unison")、 wire = swift-protobuf。
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
            ]
        ),
        .testTarget(
            name: "UnisonClientTests",
            dependencies: ["UnisonClient"]
        ),
    ]
)
