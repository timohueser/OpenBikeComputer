// swift-tools-version: 5.9
import PackageDescription

// OBCKit — the domain + transport core of the OBC companion app, kept OUTSIDE the
// app target so it builds and tests without a simulator (`swift test`).
//
// Layer order (lower may not import higher):
//   OBCDomain  → pure value types, no framework deps
//   OBCTransport → DeviceTransport protocol + (B1) BLETransport, depends on OBCDomain
//   OBCFormats → interchange file formats (route import / ride export seams, B6/B7),
//                depends on OBCDomain — sits beside OBCTransport, never on it
//   OBCMock    → #if DEBUG fixtures + MockTransport, depends on OBCTransport
//   OBCUI      → SwiftUI component kit (B11) + feature screens (B2+), depends on
//                OBCDomain + OBCTransport (feature view models consume the
//                DeviceTransport protocol — never OBCMock, never CoreBluetooth)
//
// Strict concurrency is on for every target (see `strictConcurrency` below).

// Complete concurrency checking without jumping to the Swift 6 language mode
// (issue B0: "Swift 5.9+, strict concurrency on").
let strictConcurrency: [SwiftSetting] = [
    .enableExperimentalFeature("StrictConcurrency")
]

let package = Package(
    name: "OBCKit",
    // iOS is the ship target; the macOS floor only lets host tooling (`swift test`)
    // compile the SwiftUI-using code (OBCUI, the OBCMock dev panel — whose
    // two-parameter `onChange` needs 14). The app itself is iPhone-only.
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "OBCDomain", targets: ["OBCDomain"]),
        .library(name: "OBCTransport", targets: ["OBCTransport"]),
        .library(name: "OBCFormats", targets: ["OBCFormats"]),
        .library(name: "OBCMock", targets: ["OBCMock"]),
        .library(name: "OBCUI", targets: ["OBCUI"]),
    ],
    targets: [
        .target(
            name: "OBCDomain",
            swiftSettings: strictConcurrency
        ),
        .target(
            name: "OBCTransport",
            dependencies: ["OBCDomain"],
            swiftSettings: strictConcurrency
        ),
        .target(
            name: "OBCFormats",
            dependencies: ["OBCDomain"],
            swiftSettings: strictConcurrency
        ),
        .target(
            name: "OBCMock",
            dependencies: ["OBCTransport", "OBCDomain"],
            // Editable JSON fixture sets (routes/rides/config/diagnostics) the mock
            // serves. The Swift that loads them is `#if DEBUG`; these are inert data.
            resources: [.process("Fixtures")],
            swiftSettings: strictConcurrency
        ),
        .target(
            name: "OBCUI",
            dependencies: ["OBCDomain", "OBCTransport"],
            swiftSettings: strictConcurrency
        ),
        .testTarget(
            name: "OBCTransportTests",
            // OBCFormats so the route-encoder test can decode a real GPX export
            // through the production decoder and encode it to OBCR end to end.
            dependencies: ["OBCTransport", "OBCFormats"],
            // Checked-in library files from older app versions (e.g. the v1
            // planned-route JSON) — the persistence-compat pins.
            resources: [.copy("Fixtures")],
            swiftSettings: strictConcurrency
        ),
        .testTarget(
            name: "OBCFormatsTests",
            dependencies: ["OBCFormats"],
            swiftSettings: strictConcurrency
        ),
        .testTarget(
            name: "OBCMockTests",
            dependencies: ["OBCMock"],
            swiftSettings: strictConcurrency
        ),
        .testTarget(
            name: "OBCUITests",
            // OBCMock so the launch-flow model tests drive real scenarios
            // through MockTransport (host-side, no simulator).
            dependencies: ["OBCUI", "OBCMock"],
            swiftSettings: strictConcurrency
        ),
    ]
)
