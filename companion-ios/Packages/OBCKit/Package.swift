// swift-tools-version: 6.0
import PackageDescription

// OBCKit — the domain + transport core of the OBC companion app, kept OUTSIDE the
// app target so it builds and tests without a simulator (`swift test`).
//
// Layer order (lower may not import higher):
//   OBCDomain  → pure value types, no framework deps
//   OBCWeatherWire → provider-neutral OBCW wire DTOs + codec, depends only on OBCDomain
//   OBCTransport → DeviceTransport protocol + (B1) BLETransport, depends on OBCDomain
//   OBCFormats → interchange file formats (route import / ride export seams, B6/B7),
//                depends on OBCDomain — sits beside OBCTransport, never on it
//   OBCMock    → #if DEBUG fixtures + MockTransport, depends on OBCTransport
//   OBCUI      → SwiftUI component kit (B11) + feature screens (B2+), depends on
//                OBCDomain + OBCTransport (feature view models consume the
//                DeviceTransport protocol — never OBCMock, never CoreBluetooth)
//
// Every target compiles in the Swift 6 language mode (see `languageMode` below).

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
        .library(name: "OBCTransport", targets: ["OBCTransport"]),
        .library(name: "OBCWeatherWire", targets: ["OBCWeatherWire"]),
        .library(name: "OBCFormats", targets: ["OBCFormats"]),
        .library(name: "OBCMock", targets: ["OBCMock"]),
        .library(name: "OBCUI", targets: ["OBCUI"]),
    ],
    targets: [
        .target(
            name: "OBCDomain",
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCTransport",
            dependencies: ["OBCDomain"],
            swiftSettings: languageMode
        ),
        // Provider-neutral OBCW bytes and wire DTOs. Deliberately separate from
        // CoreBluetooth/DeviceTransport so weather production can depend inward on the
        // contract and OBCTransport can consume it later without a dependency inversion.
        .target(
            name: "OBCWeatherWire",
            dependencies: ["OBCDomain"],
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
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCTransportTests",
            // OBCFormats so the route-encoder test can decode a real GPX export
            // through the production decoder and encode it to OBCR end to end.
            dependencies: ["OBCTransport", "OBCFormats"],
            // Checked-in library files from older app versions (e.g. the v1
            // planned-route JSON) — the persistence-compat pins.
            resources: [.copy("Fixtures")],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCWeatherWireTests",
            dependencies: ["OBCWeatherWire"],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCFormatsTests",
            dependencies: ["OBCFormats"],
            resources: [.copy("Fixtures")],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCMockTests",
            dependencies: ["OBCMock"],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCUITests",
            // OBCMock so the launch-flow model tests drive real scenarios
            // through MockTransport (host-side, no simulator).
            dependencies: ["OBCUI", "OBCMock"],
            swiftSettings: languageMode
        ),
    ]
)
