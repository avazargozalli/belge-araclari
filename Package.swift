// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "BelgeAraclari",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "BelgeAraclari",
            path: "Sources/BelgeAraclari"
        )
    ]
)
