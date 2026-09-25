// swift-tools-version: 6.0
import PackageDescription

// Swift 6 language mode — full data-race safety as a language guarantee, not an
// experimental flag. tools-6 defaults to v6 anyway; setting it per target keeps
// the choice explicit (and lets a single target stage back to .v5 if it ever
// has to, without flipping the whole package).
let languageMode: [SwiftSetting] = [
    .swiftLanguageMode(.v6)
]

let package = Package(
    name: "OBCKit",
    // iOS is the ship target; the macOS floor only lets host tooling (`swift test`)
    // compile the SwiftUI-using code (OBCUI, the OBCMock dev panel — whose
    // two-parameter `onChange` needs 14). The app itself is iPhone-only.
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "OBCDomain", targets: ["OBCDomain"]),
        .library(name: "OBCProtocolV4", targets: ["OBCProtocolV4"]),
        .library(name: "OBCTransport", targets: ["OBCTransport"]),

        .library(name: "OBCFormats", targets: ["OBCFormats"]),
        .library(name: "OBCMock", targets: ["OBCMock"]),
        .library(name: "OBCUI", targets: ["OBCUI"]),
    ],
    targets: [
        .target(
            name: "OBCDomain",
            swiftSettings: languageMode
        ),
        // FLAT store protocol v4, kept apart from the transports and judged by the pinned records
        // under `specs/vectors/flat-store-v4/`. The transfer client depends only on its
        // physical-link seam, so its announce → stream → result and STATUS reconcile paths are
        // host-testable.
        .target(
            name: "OBCProtocolV4",
            dependencies: ["OBCDomain"],
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCTransport",
            dependencies: ["OBCDomain", "OBCProtocolV4"],
            swiftSettings: languageMode
        ),

        .target(
            name: "OBCFormats",
            dependencies: ["OBCDomain"],
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCMock",
            dependencies: ["OBCTransport", "OBCDomain"],
            // Editable JSON fixture sets (routes/rides/config/diagnostics) the mock
            // serves. The Swift that loads them is `#if DEBUG`; these are inert data.
            resources: [.process("Fixtures")],
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCUI",
            dependencies: ["OBCDomain", "OBCTransport"],
            // The device's Terminus glyph strips, copied from `firmware/obc-render/fonts/terminus/`.
            // `PixelTextTests` fails when a copy drifts from the firmware file.
            resources: [.copy("Resources/Terminus"), .copy("Resources/Replay")],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCTransportTests",
            dependencies: ["OBCTransport", "OBCFormats"],
            // Checked-in library files from older app versions (e.g. the v1
            // planned-route JSON) — the persistence-compat pins.
            resources: [.copy("Fixtures")],
            swiftSettings: languageMode
        ),
        // Driven entirely by the checked-in protocol-v4 vectors resolved from `#filePath`.
        .testTarget(
            name: "OBCProtocolV4Tests",
            dependencies: ["OBCProtocolV4"],
            swiftSettings: languageMode
        ),

        .testTarget(
            name: "OBCFormatsTests",
            dependencies: ["OBCFormats"],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCMockTests",
            dependencies: ["OBCMock"],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCUITests",
            dependencies: ["OBCUI", "OBCMock"],
            // Recorded Apple Maps answers.
            resources: [.copy("Fixtures")],
            swiftSettings: languageMode
        ),
    ]
)
