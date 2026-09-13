import Foundation
import OBCDomain
import Testing
@testable import OBCTransport

@Suite("Durable ride archive")
struct RideArchiveTests {
    private enum Interrupted: Error { case now }
    private func directory() throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    private func ride(revision: UInt64 = 1) -> Ride {
        let storeID = String(repeating: "a", count: 32)
        let id = RideID(deviceObjectID: DeviceObjectID(41), scope: LibraryScope(serial: "test", storeID: storeID))
        let source = RideSource(storeID: storeID, objectID: 41, revision: revision,
                                payloadLength: 120, payloadCRC32: UInt32(revision))
        let summary = RideSummary(id: id, name: "Ride \(revision)", date: Date(timeIntervalSince1970: 100),
                                  distanceMeters: Double(revision), avgHeartRate: 120, maxHeartRate: 180,
                                  avgCadence: 75, avgPower: 180, maxPower: 390, source: source)
        return Ride(summary: summary, points: [
            RidePoint(timestamp: summary.date, coordinate: Coordinate(latitude: 48, longitude: 9),
                      elevationMeters: nil, heartRate: 130, cadence: 80, power: 190, segmentStart: true),
            RidePoint(timestamp: summary.date.addingTimeInterval(1), coordinate: Coordinate(latitude: 48.1, longitude: 9),
                      elevationMeters: 410, heartRate: nil, cadence: nil, power: nil, segmentStart: false),
        ])
    }

    @Test func archiveReopensWithAllFieldsAndExactSource() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = FileLibraryStore(directory: dir)
        let ride = ride()
        let receipt = try #require(try store.archiveRide(ride))
        #expect(receipt.source == ride.summary.source)
        let reopened = FileLibraryStore(directory: dir)
        #expect(reopened.rideSummaries() == [ride.summary])
        #expect(reopened.ridePoints(ride.id) == ride.points)
        #expect(reopened.archivedRideSource(ride.id) == ride.summary.source)
        #expect(reopened.syncedRideIDs() == [ride.id])
        var renamed = ride.summary
        renamed.name = "Local name"
        reopened.saveRideSummary(renamed)
        #expect(reopened.archivedRideSource(ride.id) == receipt.source)
        #expect(reopened.ridePoints(ride.id) == ride.points)
    }

    @Test(arguments: FileLibraryStore.ArchiveCheckpoint.allCases)
    func interruptedReplacementNeverMixesGenerations(checkpoint: FileLibraryStore.ArchiveCheckpoint) throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let old = ride()
        let next = ride(revision: 2)
        let store = FileLibraryStore(directory: dir)
        _ = try store.archiveRide(old)
        let interrupted = FileLibraryStore(directory: dir) { operation in
            if operation == checkpoint { throw Interrupted.now }
        }
        #expect(throws: Interrupted.self) { try interrupted.archiveRide(next) }
        let visible = checkpoint == .directorySync ? next : old
        #expect(interrupted.rideSummaries() == [visible.summary])
        #expect(interrupted.ridePoints(old.id) == visible.points)
        if checkpoint == .directorySync {
            #expect(interrupted.archivedRideSource(old.id) == nil)
        }
        let reopened = FileLibraryStore(directory: dir)
        #expect(reopened.archivedRideSource(old.id) == visible.summary.source)
        _ = try reopened.archiveRide(next)
        #expect(reopened.rideSummaries() == [next.summary])
        #expect(reopened.archivedRideSource(next.id) == next.summary.source)
    }

    @Test func historyAndMissingPointsAreNotArchiveProof() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = FileLibraryStore(directory: dir)
        let ride = ride()
        store.markRideSynced(ride.id)
        #expect(store.archivedRideSource(ride.id) == nil)
        _ = try store.archiveRide(ride)
        let rideDir = try #require(FileManager.default.contentsOfDirectory(
            at: dir.appendingPathComponent("rides"), includingPropertiesForKeys: nil).first)
        let points = try #require(FileManager.default.contentsOfDirectory(
            at: rideDir, includingPropertiesForKeys: nil).first { $0.lastPathComponent.hasPrefix("points-") })
        try Data("corrupt".utf8).write(to: points)
        #expect(store.archivedRideSource(ride.id) == nil)
        try FileManager.default.removeItem(at: points)
        #expect(store.archivedRideSource(ride.id) == nil)
        #expect(store.rideSummaries() == [ride.summary])
    }

    @Test func missingCustomAnchorFailsBeforeArchiveCreation() throws {
        let root = try directory()
        defer { try? FileManager.default.removeItem(at: root) }
        let anchor = root.appendingPathComponent("missing-parent")
        let archive = anchor.appendingPathComponent("library")
        let store = FileLibraryStore(directory: archive)
        #expect(throws: RideArchiveError.unreadableArchive) { try store.archiveRide(ride()) }
        #expect(!FileManager.default.fileExists(atPath: archive.path))
        try FileManager.default.createDirectory(at: anchor, withIntermediateDirectories: false)
        _ = try store.archiveRide(ride())
        #expect(store.archivedRideSource(ride().id) == ride().summary.source)
    }

    @Test func sourceMismatchAndUnreadableManifestStayProtected() throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = FileLibraryStore(directory: dir)
        var ride = ride()
        ride.summary.source = RideSource(storeID: String(repeating: "b", count: 32), objectID: 41,
                                         revision: 1, payloadLength: 1, payloadCRC32: 1)
        #expect(throws: RideArchiveError.invalidSource) { try store.archiveRide(ride) }
        ride = self.ride()
        _ = try store.archiveRide(ride)
        let rideDir = try #require(FileManager.default.contentsOfDirectory(
            at: dir.appendingPathComponent("rides"), includingPropertiesForKeys: nil).first)
        let manifest = rideDir.appendingPathComponent("summary.json")
        let protected = Data("unsupported or damaged archive".utf8)
        try protected.write(to: manifest)
        #expect(throws: RideArchiveError.unreadableArchive) { try store.archiveRide(ride) }
        store.saveRideSummary(ride.summary)
        #expect(try Data(contentsOf: manifest) == protected)
    }
}
