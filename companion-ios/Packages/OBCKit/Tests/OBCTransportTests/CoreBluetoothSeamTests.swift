import XCTest

/// The epic's central invariant, enforced (B1 acceptance: "zero CoreBluetooth
/// references outside `BLETransport`"). No iOS CI runs yet, so this lives as a
/// `swift test` guard — the moment any iOS CI is wired, it enforces the seam. It
/// scans source files on disk rather than symbols so it also catches comments/docs.
final class CoreBluetoothSeamTests: XCTestCase {
    private let needle = ["import", "CoreBluetooth"].joined(separator: " ")

    func testCoreBluetoothConfinedToTheBLEFolder() throws {
        let sources = packageRoot().appendingPathComponent("Sources")
        let offenders = swiftFiles(under: sources).filter {
            fileContains($0, needle) && !$0.path.contains("/BLE/")
        }
        XCTAssertTrue(
            offenders.isEmpty,
            "CoreBluetooth must live only in OBCTransport/BLE/. Offenders: \(offenders.map(\.lastPathComponent))"
        )
    }

    func testAppCompositionRootNeverImportsCoreBluetooth() throws {
        let appDir = companionRoot().appendingPathComponent("OBCCompanion")
        guard FileManager.default.fileExists(atPath: appDir.path) else {
            throw XCTSkip("app target not present relative to the package")
        }
        let offenders = swiftFiles(under: appDir).filter { fileContains($0, needle) }
        XCTAssertTrue(
            offenders.isEmpty,
            "The composition root must see only DeviceTransport. Offenders: \(offenders.map(\.lastPathComponent))"
        )
    }

    // MARK: Helpers

    private func swiftFiles(under directory: URL) -> [URL] {
        guard let enumerator = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil) else {
            return []
        }
        return enumerator.compactMap { $0 as? URL }.filter { $0.pathExtension == "swift" }
    }

    private func fileContains(_ url: URL, _ text: String) -> Bool {
        (try? String(contentsOf: url, encoding: .utf8))?.contains(text) ?? false
    }

    private func packageRoot() -> URL {
        // .../Packages/OBCKit/Tests/OBCTransportTests/CoreBluetoothSeamTests.swift → OBCKit
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    private func companionRoot() -> URL {
        // OBCKit → Packages → companion-ios
        packageRoot().deletingLastPathComponent().deletingLastPathComponent()
    }
}
