// swift-tools-version: 6.0
import PackageDescription

// OBCKit — the domain + transport core of the OBC companion app, kept OUTSIDE the
// app target so it builds and tests without a simulator (`swift test`).
//
// Layer order (lower may not import higher):
//   OBCDomain  → pure value types, no framework deps
//   OBCWeatherWire → provider-neutral OBCW wire DTOs + codec, depends only on OBCDomain
//   OBCWeather → the weather domain: MET hourly adapter, OBC weather service client
//                and the OBCW bundle builder. Depends on OBCDomain + OBCWeatherWire,
//                never on OBCTransport (no CoreBluetooth anywhere near it) and never
//                on SwiftUI
//   OBCTransport → DeviceTransport protocol + (B1) BLETransport, depends on OBCDomain
//                + OBCWeather (WX9: the transport conforms to the weather job's
//                `WeatherDeviceLink` seam — the dependency points *down* into the
//                weather domain, never the other way around)
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
        .library(name: "OBCWeather", targets: ["OBCWeather"]),
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
            // OBCWeather sits *below* the transport (a domain-layer package like OBCDomain): the
            // WX9 job seam (`Weather/WeatherBLEDeviceLink.swift`) conforms the transport to
            // OBCWeather's `WeatherDeviceLink` protocol. The direction that matters is unchanged —
            // OBCWeather never imports OBCTransport, so no weather code can reach CoreBluetooth.
            dependencies: ["OBCDomain", "OBCWeather"],
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
        // The weather domain: semantic Sendable state, the MET Norway hourly adapter, the
        // OBC weather service client (manifest + corridor Range reads) and the OBCW builder.
        // Sits on OBCDomain + OBCWeatherWire so provider formats end at its adapters and the
        // bytes it emits are the frozen wire contract, never a second one.
        .target(
            name: "OBCWeather",
            dependencies: ["OBCDomain", "OBCWeatherWire"],
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCFormats",
            dependencies: ["OBCDomain"],
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCMock",
            // OBCWeather for the WX13 weather fixtures (job ring, service status, owed job) the
            // Weather screens are driven and photographed against.
            dependencies: ["OBCTransport", "OBCDomain", "OBCWeather"],
            // Editable JSON fixture sets (routes/rides/config/diagnostics) the mock
            // serves. The Swift that loads them is `#if DEBUG`; these are inert data.
            resources: [.process("Fixtures")],
            swiftSettings: languageMode
        ),
        .target(
            name: "OBCUI",
            // OBCWeather for WX13's weather settings / diagnostics / privacy screens: they render
            // the job history ring, the manifest-sourced attribution and the pending job's phase.
            // No layer order changes — OBCWeather already sits *below* OBCTransport, which OBCUI
            // depends on; naming it explicitly keeps the import legal rather than transitively
            // lucky.
            dependencies: ["OBCDomain", "OBCTransport", "OBCWeather"],
            swiftSettings: languageMode
        ),
        .testTarget(
            name: "OBCTransportTests",
            // OBCFormats so the route-encoder test can decode a real GPX export
            // through the production decoder and encode it to OBCR end to end.
            // OBCWeather so the WX9 context → snapshot mapping is pinned against
            // the job engine's own types.
            dependencies: ["OBCTransport", "OBCFormats", "OBCWeather"],
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
            name: "OBCWeatherTests",
            dependencies: ["OBCWeather"],
            // The WX1 MET captures — rehomed here when WX6 deleted the source spike crate that
            // used to carry them (the suite still pins the adapter against real provider bytes).
            resources: [.copy("Fixtures")],
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
            // OBCMock so the launch-flow model tests drive real scenarios
            // through MockTransport (host-side, no simulator). OBCWeather so the WX13 weather
            // settings/diagnostics model tests build history rings and service statuses from the
            // production types.
            dependencies: ["OBCUI", "OBCMock", "OBCWeather"],
            swiftSettings: languageMode
        ),
    ]
)
