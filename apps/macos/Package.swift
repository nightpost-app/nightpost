// swift-tools-version: 6.0
// The macOS interface: SwiftUI, a client of the same daemon as the others
// (crates/cli) through its local API. No logic of its own.
import PackageDescription

let package = Package(
    name: "NightpostMac",
    platforms: [.macOS(.v14)],
    targets: [
        // Not "Nightpost": the daemon, "nightpost", sits beside it in the
        // bundle, and macOS file names ignore case.
        .executableTarget(name: "NightpostMac", path: "Sources/NightpostMac")
    ]
)
