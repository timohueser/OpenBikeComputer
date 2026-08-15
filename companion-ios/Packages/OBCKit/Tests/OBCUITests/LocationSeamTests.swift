import XCTest

/// The privacy page's hardest claim, enforced rather than asserted (WX13 / #1198).
///
/// `WeatherPrivacyCopy` tells the rider: *"No location permission from this phone: the app never
/// asks iOS where you are."* A test that only checks the sentence is present checks that somebody
/// typed it. This checks the thing it claims — the same shape as `CoreBluetoothSeamTests`, which
/// scans files on disk rather than symbols so it also catches a comment, a doc line or a
/// half-finished experiment.
///
/// Two gates, because the claim has two halves:
///
/// - **No CoreLocation API.** `import CoreLocation` and `CLLocationManager` are the two ways an
///   app asks iOS for a position; neither may appear anywhere in the package or the app target.
///   `CLLocationCoordinate2D` is *not* forbidden — it is a plain latitude/longitude struct the
///   MapKit views legitimately hand back and forth, and it asks nobody for anything.
/// - **No usage-description key.** This is the hard one: without an `NSLocation*` key in the
///   Info.plist, iOS cannot grant location to this app at all — the authorisation request fails
///   silently and the OS never shows a prompt. Code can be added by anyone; the key is the
///   platform-level guarantee behind the sentence, and it lives in `project.yml`, which is the
///   source of truth for the plist (the plist itself is generated and gitignored).
final class LocationSeamTests: XCTestCase {
    /// Split so this file's own source cannot match the needles it scans for.
    private let importNeedle = ["import", "CoreLocation"].joined(separator: " ")
    private let managerNeedle = ["CL", "LocationManager"].joined()
    private let plistNeedle = ["NS", "Location"].joined()

    func testNoLocationAPIAnywhereInThePackage() throws {
        let sources = packageRoot().appendingPathComponent("Sources")
        let offenders = swiftFiles(under: sources).filter {
            fileContains($0, importNeedle) || fileContains($0, managerNeedle)
        }
        XCTAssertTrue(
            offenders.isEmpty,
            """
            The weather privacy page promises the app never asks iOS where you are. \
            Offenders: \(offenders.map(\.lastPathComponent))
            """
        )
    }

    func testNoLocationAPIInTheAppTarget() throws {
        let appDir = companionRoot().appendingPathComponent("OBCCompanion")
        guard FileManager.default.fileExists(atPath: appDir.path) else {
            throw XCTSkip("app target not present relative to the package")
        }
        let offenders = swiftFiles(under: appDir).filter {
            fileContains($0, importNeedle) || fileContains($0, managerNeedle)
        }
        XCTAssertTrue(
            offenders.isEmpty,
            "The composition root must not reach for CoreLocation either. "
                + "Offenders: \(offenders.map(\.lastPathComponent))"
        )
    }

    /// The map views legitimately use MapKit's coordinate value type; forbidding it would forbid
    /// drawing a track. Pinned so a later tightening of the needles cannot quietly ban it — and so
    /// the gate above is visibly *not* vacuous.
    func testTheMapKitCoordinateValueTypeIsStillAllowed() throws {
        let sources = packageRoot().appendingPathComponent("Sources")
        let users = swiftFiles(under: sources).filter {
            fileContains($0, ["CL", "LocationCoordinate2D"].joined())
        }
        XCTAssertFalse(users.isEmpty, "expected the Map views to use CLLocationCoordinate2D")
    }

    /// Without an `NSLocation*` usage description iOS cannot grant location, whatever the code
    /// asks for. `project.yml` is where the app's Info.plist is written (the plist is generated and
    /// gitignored), so it is where the guarantee has to hold.
    func testNoLocationUsageDescriptionKeyIsDeclared() throws {
        var scanned: [URL] = []
        var offenders: [URL] = []
        let root = companionRoot()
        let candidates =
            [root.appendingPathComponent("project.yml"),
             root.appendingPathComponent("project.local.yml")]
            + plists(under: root.appendingPathComponent("OBCCompanion"))
        for url in candidates where FileManager.default.fileExists(atPath: url.path) {
            scanned.append(url)
            if fileContains(url, plistNeedle) { offenders.append(url) }
        }
        XCTAssertTrue(
            scanned.contains { $0.lastPathComponent == "project.yml" },
            "project.yml not found — this gate must not pass by scanning nothing")
        XCTAssertTrue(
            offenders.isEmpty,
            """
            A location usage-description key would let iOS grant this app a position, which the \
            weather privacy page says it never asks for. Offenders: \
            \(offenders.map(\.lastPathComponent))
            """
        )
    }

    // MARK: Helpers

    private func swiftFiles(under directory: URL) -> [URL] {
        files(under: directory) { $0.pathExtension == "swift" }
    }

    private func plists(under directory: URL) -> [URL] {
        files(under: directory) { $0.pathExtension == "plist" }
    }

    private func files(under directory: URL, matching: (URL) -> Bool) -> [URL] {
        guard let enumerator = FileManager.default.enumerator(
            at: directory, includingPropertiesForKeys: nil)
        else { return [] }
        return enumerator.compactMap { $0 as? URL }.filter(matching)
    }

    private func fileContains(_ url: URL, _ text: String) -> Bool {
        (try? String(contentsOf: url, encoding: .utf8))?.contains(text) ?? false
    }

    private func packageRoot() -> URL {
        // .../Packages/OBCKit/Tests/OBCUITests/LocationSeamTests.swift → OBCKit
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
