// swift-tools-version: 6.0
import PackageDescription

// EchoHarness — a macOS command-line rig that drives the firmware's A5 L2CAP CoC echo
// loopback (issue #273) from the terminal, reusing the *actual* app transport code:
// `OBCTransport`'s `BLEChannel` byte layer, `L2CAPByteChannel`, the `TransferControl` /
// `StatusMessage` descriptors, `CRC32`, and the `GATT` UUID map. macOS has CoreBluetooth
// incl. `CBL2CAPChannel`, so the harness opens a real CoC and echo-round-trips objects —
// the A5 oracle that isn't the iOS app (so failures localize), and the seed the A9 soak
// rig grows from. Not part of the iOS app build; run with `swift run` on a Mac.
let package = Package(
    name: "EchoHarness",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "echo-harness", targets: ["EchoHarness"])
    ],
    dependencies: [
        .package(path: "../Packages/OBCKit")
    ],
    targets: [
        .executableTarget(
            name: "EchoHarness",
            dependencies: [
                .product(name: "OBCTransport", package: "OBCKit"),
                // OBCDomain for `DeviceObjectID` — the trip codecs (`TripObjectCodec`, `TripList`)
                // speak device object ids, which the trip-soak scenario constructs directly.
                .product(name: "OBCDomain", package: "OBCKit"),
            ],
            swiftSettings: [.swiftLanguageMode(.v6)]
        )
    ]
)
