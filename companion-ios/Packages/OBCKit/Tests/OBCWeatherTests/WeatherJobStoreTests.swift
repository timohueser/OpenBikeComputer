import Foundation
import Testing
@testable import OBCWeather

// The WX9 checkpoint and history persistence: real files in a temp directory, because the whole
// point of these stores is surviving a process that does not.

private func temporaryDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("obc-wx9-tests-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@Suite("Weather job checkpoint store")
struct FileWeatherJobStoreTests {
    @Test func aFullRecordRoundTripsThroughDisk() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let readAt = Date(timeIntervalSince1970: 1_770_000_000)
        let store = FileWeatherJobStore(
            fileURL: directory.appendingPathComponent("job.json"), now: { readAt })

        let record = WeatherJobRecord(
            phase: .bundleReady,
            snapshot: WeatherDeviceRequestSnapshot(
                requestID: 42, latitudeMicrodegrees: -49_330_889, longitudeMicrodegrees: -72_886_121,
                fixUnixSeconds: 1_769_999_990, bearingDegrees: 271, speedMetresPerSecond: 6.4,
                routeID: 3, heldBundleGeneration: 17,
                heldBundleGeneratedAtUnixSeconds: 1_769_000_000, reasonRawValue: 0b101,
                readAt: readAt),
            bundleBytes: Data([0xAA, 0xBB]), bundleGeneration: 18,
            bundleWindow: [-50_000_000, -73_000_000, -49_000_000, -72_000_000],
            bundleBuiltAt: readAt, precipitationGeneration: "mrms", noRainMapReason: nil,
            attempts: 2, startedAt: readAt, updatedAt: readAt,
            notBefore: readAt.addingTimeInterval(30))
        store.save(record)

        // A second store over the same file is the relaunch.
        let reloaded = FileWeatherJobStore(
            fileURL: directory.appendingPathComponent("job.json"), now: { readAt }).load()
        // Dates round-trip through secondsSince1970, so compare the whole values.
        #expect(reloaded == record)

        store.clear()
        #expect(store.load() == nil)
    }

    @Test func anUnreadableCheckpointIsAFreshStartNotACrashLoop() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let fileURL = directory.appendingPathComponent("job.json")
        try Data("not json at all".utf8).write(to: fileURL)
        #expect(FileWeatherJobStore(fileURL: fileURL).load() == nil)
    }

    /// The rider coordinate lives in this file and nowhere else, so it must not survive the job's
    /// own lifetime on disk waiting for an engine run that may never come.
    @Test func aCheckpointPastItsLifetimeIsRefusedAndDeleted() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let fileURL = directory.appendingPathComponent("job.json")
        let startedAt = Date(timeIntervalSince1970: 1_770_000_000)
        FileWeatherJobStore(fileURL: fileURL, lifetime: 3_600, now: { startedAt }).save(
            WeatherJobRecord(
                phase: .bundleReady,
                snapshot: WeatherDeviceRequestSnapshot(
                    requestID: 9, latitudeMicrodegrees: 47_500_000,
                    longitudeMicrodegrees: 7_600_000, readAt: startedAt),
                startedAt: startedAt, updatedAt: startedAt))
        #expect(FileManager.default.fileExists(atPath: fileURL.path))

        // Inside the horizon it still resumes…
        let fresh = FileWeatherJobStore(
            fileURL: fileURL, lifetime: 3_600, now: { startedAt.addingTimeInterval(3_000) })
        #expect(fresh.load()?.snapshot?.requestID == 9)

        // …past it the load refuses *and* takes the coordinate off disk.
        let stale = FileWeatherJobStore(
            fileURL: fileURL, lifetime: 3_600, now: { startedAt.addingTimeInterval(3_601) })
        #expect(stale.load() == nil)
        #expect(!FileManager.default.fileExists(atPath: fileURL.path))
    }
}

@Suite("Weather job history ring")
struct FileWeatherJobHistoryStoreTests {
    private func entry(_ requestID: UInt32) -> WeatherJobHistoryEntry {
        WeatherJobHistoryEntry(
            startedAt: Date(timeIntervalSince1970: 1_770_000_000),
            finishedAt: Date(timeIntervalSince1970: 1_770_000_100),
            requestID: requestID, outcome: .committed, phaseReached: .uploading, attempts: 1)
    }

    @Test func theRingKeepsOnlyTheNewestEntries() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = FileWeatherJobHistoryStore(
            fileURL: directory.appendingPathComponent("history.json"), capacity: 3)
        for id in 1...5 { store.append(entry(UInt32(id))) }
        #expect(store.entries().map(\.requestID) == [3, 4, 5])
    }

    @Test func entriesSurviveARelaunch() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let fileURL = directory.appendingPathComponent("history.json")
        FileWeatherJobHistoryStore(fileURL: fileURL).append(entry(7))
        let reloaded = FileWeatherJobHistoryStore(fileURL: fileURL).entries()
        #expect(reloaded.map(\.requestID) == [7])
        #expect(reloaded.first?.outcome == .committed)
    }

    /// A ring written by a previous build must cost the rows *that build* wrote differently — not
    /// the whole ring. `noRainMapReason` was a `String` in the WX9-era ring and is a
    /// ``NoRainMapReason`` now; decoding the array as a unit meant one such row silently wiped a
    /// rider's entire sync history the first time they opened the updated app (#1198 review).
    @Test func oneUnreadableLegacyRowDoesNotWipeTheRing() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let fileURL = directory.appendingPathComponent("history.json")

        // A ring as the previous build left it: two rows this build reads fine, and between them
        // one carrying the old string-shaped reason.
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        func object(_ row: WeatherJobHistoryEntry) throws -> [String: Any] {
            let data = try encoder.encode(row)
            return try JSONSerialization.jsonObject(with: data) as! [String: Any]
        }
        var legacy = try object(entry(8))
        legacy["noRainMapReason"] =
            "allCoveringProductsExpired(latestDeadline: 2026-08-10 12:00:00 +0000)"
        let onDisk: [Any] = [try object(entry(7)), legacy, try object(entry(9))]
        try JSONSerialization.data(withJSONObject: onDisk).write(to: fileURL)

        let store = FileWeatherJobHistoryStore(fileURL: fileURL)
        #expect(store.entries().map(\.requestID) == [7, 9], "only the unreadable row is dropped")

        // …and the ring stays usable: the next append lands beside the survivors rather than on a
        // file the store has given up on.
        store.append(entry(10))
        #expect(store.entries().map(\.requestID) == [7, 9, 10])
    }

    /// The salvage path is a fallback, not a licence to invent rows: a file that is not an array
    /// at all is not a ring, and reads as empty rather than as something half-understood.
    @Test func aFileThatIsNotARingReadsAsEmpty() throws {
        let directory = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let fileURL = directory.appendingPathComponent("history.json")
        try Data("not json at all".utf8).write(to: fileURL)
        #expect(FileWeatherJobHistoryStore(fileURL: fileURL).entries().isEmpty)
    }
}
