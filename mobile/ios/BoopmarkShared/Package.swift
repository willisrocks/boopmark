// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "BoopmarkShared",
    platforms: [
        .iOS(.v16),
        .macOS(.v13)
    ],
    products: [
        .library(name: "BoopmarkShared", targets: ["BoopmarkShared"])
    ],
    targets: [
        .target(name: "BoopmarkShared"),
        .testTarget(
            name: "BoopmarkSharedTests",
            dependencies: ["BoopmarkShared"]
        )
    ]
)
