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
        let store = FileWeatherJobStore(fileURL: directory.appendingPathComponent("job.json"))

        let readAt = Date(timeIntervalSince1970: 1_770_000_000)
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
            bundleBuiltAt: readAt, precipitationProductID: "mrms", noRainMapReason: nil,
            attempts: 2, startedAt: readAt, updatedAt: readAt,
            notBefore: readAt.addingTimeInterval(30))
        store.save(record)

        // A second store over the same file is the relaunch.
        let reloaded = FileWeatherJobStore(fileURL: directory.appendingPathComponent("job.json")).load()
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
}
