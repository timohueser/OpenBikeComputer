// swift-tools-version: 6.0
import Foundation
import PackageDescription

// The device's router on the phone, linked from Rust (`apps/obc-companion-core`). It is its own
// package so that OBCKit builds without Rust: only the app and this package's tests need the core.
let xcframework = "../../../target/OBCCompanionCore.xcframework"

// SwiftPM's own error for a missing binary artifact names no fix.
let manifestDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
guard FileManager.default.fileExists(atPath: manifestDirectory.appending(path: xcframework).path) else {
    fatalError("target/OBCCompanionCore.xcframework is missing: run `obc companion-core` first")
}

let package = Package(
    name: "OBCRouting",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "OBCRouting", targets: ["OBCRouting"]),
    ],
    dependencies: [
        .package(path: "../OBCKit"),
    ],
    targets: [
        .binaryTarget(name: "OBCCompanionCore", path: xcframework),
        .target(
            name: "OBCRouting",
            dependencies: [.product(name: "OBCDomain", package: "OBCKit"), "OBCCompanionCore"],
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
        // Reads the web builder's cell fixture and the Rust crate's route vector from `#filePath`.
        .testTarget(
            name: "OBCRoutingTests",
            dependencies: ["OBCRouting"],
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
    ]
)
