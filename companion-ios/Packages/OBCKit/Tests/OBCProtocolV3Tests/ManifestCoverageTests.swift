import Foundation
import Testing

@testable import OBCProtocolV3

/// The drift guard. `Device_Object_Vectors_v2.md` §7 requires "checked-in fixture hashes and a CI
/// guard that fails on an unreviewed fixture rewrite" — plus, for this slice, proof that no fixture
/// is quietly unrepresented in Swift.
///
/// The guard is deliberately self-sufficient: it walks `manifest.json`, runs the *same* exerciser
/// the suites above use over every row, and then asserts the exercise log covers every name. That
/// makes it independent of Swift Testing's suite scheduling — a fixture cannot pass by being run
/// somewhere else, and it cannot be skipped by a suite quietly dropping an argument.
@Suite("Device Object v3 — manifest drift guard")
struct ManifestCoverageTests {
    @Test("the manifest pins this suite, this format, and wire major 3")
    func manifestIdentity() {
        #expect(DeviceObjectVectors.manifest.suite == "device-object-v2")
        #expect(DeviceObjectVectors.manifest.format == 1)
        #expect(DeviceObjectVectors.manifest.wireMajor == Int(WireLimits.major))
        #expect(DeviceObjectVectors.manifest.storageFormat == 1)
    }

    @Test("every manifest row exists, hashes to its recorded SHA-256, and is exercised by Swift")
    func everyManifestRowIsCovered() throws {
        let entries = DeviceObjectVectors.manifest.allEntries
        #expect(!entries.isEmpty)

        var failures: [String] = []
        for entry in entries {
            #expect(!entry.name.isEmpty, "manifest row with no name")
            #expect(!entry.file.isEmpty, "manifest row \(entry.name) with no file")
            #expect(entry.sha256.count == 64, "manifest row \(entry.name) with no SHA-256")
            do {
                try VectorExerciser.exercise(entry)
            } catch {
                failures.append("\(entry): \(error)")
            }
        }
        #expect(failures.isEmpty, "\(failures.count) fixture(s) failed:\n\(failures.joined(separator: "\n"))")

        let uncovered = entries.filter { !ExerciseLog.shared.contains($0.name) }
        #expect(
            uncovered.isEmpty,
            "manifest rows with no Swift coverage: \(uncovered.map(\.description).joined(separator: ", "))")
    }

    @Test("no fixture file on disk is missing from the manifest")
    func noOrphanFixtures() throws {
        let listed = Set(DeviceObjectVectors.manifest.allEntries.map(\.file))
        var onDisk: Set<String> = []
        let root = DeviceObjectVectors.suiteDirectory
        guard
            let walker = FileManager.default.enumerator(
                at: root, includingPropertiesForKeys: nil)
        else {
            Issue.record("cannot enumerate \(root.path)")
            return
        }
        for case let url as URL in walker where url.pathExtension == "json" {
            let relative = url.path.replacingOccurrences(of: root.path + "/", with: "")
            if relative == "manifest.json" { continue }
            onDisk.insert(relative)
        }
        #expect(
            onDisk.subtracting(listed).isEmpty,
            "fixtures on disk but not in the manifest: \(onDisk.subtracting(listed).sorted())")
        #expect(
            listed.subtracting(onDisk).isEmpty,
            "manifest rows with no file: \(listed.subtracting(onDisk).sorted())")
    }

    /// §6's storage fixtures are OBC2 on-card records, and there is deliberately no Swift codec for
    /// them: the format is private to `CardStore` and no client ever sees it. What this side pins
    /// is that they are present, indexed, and *reconstructable* — the exerciser rebuilds each case
    /// from its runs and checks the digest — because a fixture only Rust can read is not a
    /// cross-language contract.
    @Test("the storage section is populated and every row is reconstructable")
    func storageSectionIsCarried() throws {
        let storage = DeviceObjectVectors.manifest.sections["storage"] ?? []
        #expect(!storage.isEmpty, "the storage section is empty; §6 fixtures land with the storage slice")
        for entry in storage {
            #expect(entry.file.hasPrefix("storage/"), "\(entry) is not under storage/")
            try VectorExerciser.exercise(entry)
        }
    }
}
