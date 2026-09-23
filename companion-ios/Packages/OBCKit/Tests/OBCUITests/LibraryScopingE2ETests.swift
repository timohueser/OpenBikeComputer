import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// Drive scoped device catalogs through the main model and sync coordinator.
/// Device changes isolate current sync from earlier scopes and local archives.
@MainActor @Suite struct LibraryScopingE2ETests {
    // MARK: The stub device

    /// A device with a mutable identity and ride store. Its `listRides()` mints ids scoped to
    /// the identity the last `deviceInfo()` read returned, like the real transport.
    final class ScopedStubDevice: DeviceLink, DeviceBattery, DeviceObjects, DeviceClock,
        @unchecked Sendable {
        private let stateMulticast = AsyncMulticast<ConnectionState>(.connected)
        private let lock = NSLock()
        private var _serial: String
        private var _storeID: String?
        private var _rides: [Ride]
        private var _downloads: [[RideID]] = []
        private var _failIdentityRead = false

        init(serial: String, storeID: String?, rides: [Ride] = []) {
            _serial = serial
            _storeID = storeID
            _rides = rides
        }

        // Test knobs.
        func setIdentity(serial: String? = nil, storeID: String?) {
            lock.withLock {
                if let serial { _serial = serial }
                _storeID = storeID
            }
        }
        func setRides(_ rides: [Ride]) { lock.withLock { _rides = rides } }
        func setFailIdentityRead(_ fail: Bool) { lock.withLock { _failIdentityRead = fail } }
        var downloadRequests: [[RideID]] { lock.withLock { _downloads } }
        var scope: LibraryScope? {
            lock.withLock { _storeID.map { LibraryScope(serial: _serial, storeID: $0) } }
        }
        func bounce() {
            stateMulticast.send(.disconnected)
            stateMulticast.send(.connected)
        }

        // Main-screen capabilities.
        var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
        var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
        func connect() async throws {}
        func disconnect() async {}

        func deviceInfo() async throws -> DeviceInfo {
            let (serial, storeID, fail) = lock.withLock { (_serial, _storeID, _failIdentityRead) }
            if fail { throw DeviceError.readFailed }
            return DeviceInfo(name: "Trailhead", firmwareVersion: "2.0", serial: serial,
                              storeID: storeID)
        }

        func listRoutes() async throws -> [RouteCatalogEntry] { [] }
        func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
            throw DeviceError.readFailed
        }
        func uploadRoute(_ route: RouteBlob) -> TransferHandle {
            .immediatelyFinished(.failed(.notConnected))
        }
        func deleteRoute(_ id: DeviceObjectID) async throws {}
        /// Scoped minting, like `BLETransport.listRides()`.
        func listRides() async throws -> RideCatalog {
            let (rides, scope) = lock.withLock {
                (_rides, _storeID.map { LibraryScope(serial: _serial, storeID: $0) })
            }
            return RideCatalog(rides: rides.map { ride in
                var summary = ride.summary
                if let objectID = summary.id.deviceObjectID, let scope {
                    summary = RideSummary(
                        id: RideID(deviceObjectID: objectID, scope: scope),
                        name: summary.name, date: summary.date,
                        distanceMeters: summary.distanceMeters,
                        movingTime: summary.movingTime,
                        averageSpeedMps: summary.averageSpeedMps,
                        climbMeters: summary.climbMeters)
                }
                return summary
            })
        }

        func downloadRides(_ ids: [RideID]) -> RideDownload {
            let rides = lock.withLock {
                _downloads.append(ids)
                return _rides
            }
            let (stream, continuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
            for id in ids {
                guard let objectID = id.deviceObjectID,
                    let ride = rides.first(where: { $0.summary.id.deviceObjectID == objectID })
                else { continue }
                continuation.yield(DownloadedRide(id: id, payload: RideObjectCodec.encode(ride.ride(withID: id))))
            }
            continuation.finish()
            return RideDownload(handle: .immediatelyFinished(.completed), rides: stream)
        }
    }

    // MARK: Helpers

    private let store1 = "1111111111111111111111110bc00001"
    private let store2 = "2222222222222222222222220bc00001"
    private let serial = "OBC-24-000317"

    /// A device-side ride under a bare object id (the device's own namespace).
    private func deviceRide(_ objectID: UInt16, name: String, start: TimeInterval) -> Ride {
        Ride(
            summary: RideSummary(
                id: RideID(deviceObjectID: DeviceObjectID(objectID)), name: name,
                date: Date(timeIntervalSince1970: start), distanceMeters: 20_000),
            points: [RidePoint(timestamp: Date(timeIntervalSince1970: start),
                               coordinate: Coordinate(latitude: 48, longitude: 8))])
    }

    private func makeModel(
        device: ScopedStubDevice, library: any LibraryStore
    ) -> MainScreenModel {
        MainScreenModel(
            transport: device, library: library,
            syncTiming: .init(syncDoneHold: .seconds(300), syncedLineHold: .seconds(300)))
    }

    // MARK: End-to-end composite keying

    /// Sync stores scoped rides once; a later sync does not download them again.
    @Test func syncLandsRidesUnderCompositeKeys() async throws {
        let device = ScopedStubDevice(
            serial: serial, storeID: store1,
            rides: [deviceRide(1, name: "Dawn Patrol", start: 1_700_000_000),
                    deviceRide(2, name: "Gravel Hour", start: 1_700_100_000)])
        let library = InMemoryLibraryStore()
        let model = makeModel(device: device, library: library)
        model.start()
        try await waitFor("identity settles") { model.connectedScope != nil }

        model.sync.sync()
        try await waitFor("both rides land") { library.rideSummaries().count == 2 }

        let scope = LibraryScope(serial: serial, storeID: store1)
        let expected: Set<RideID> = [
            RideID(deviceObjectID: DeviceObjectID(1), scope: scope),
            RideID(deviceObjectID: DeviceObjectID(2), scope: scope),
        ]
        #expect(Set(library.rideSummaries().map(\.id)) == expected)
        #expect(library.syncedRideIDs() == expected)
        // The rows carry their content, not just their keys.
        #expect(Set(library.rideSummaries().map(\.name)) == ["Dawn Patrol", "Gravel Hour"])
        #expect(library.ridePoints(RideID(deviceObjectID: DeviceObjectID(1), scope: scope))?.isEmpty == false)

        try await waitFor("sync completes") { model.sync.syncState == .done }
        model.sync.sync()
        try await waitFor("repeat sync is up to date") { model.sync.syncState == .idle }
        #expect(device.downloadRequests.count == 1)
        #expect(Set(device.downloadRequests[0]) == expected)
        #expect(Set(library.rideSummaries().map(\.id)) == expected)

    }

    /// A replacement store can reuse object IDs while earlier rides remain archival.
    @Test func eraChangeSyncsTheNewErasRidesAndKeepsTheOldArchival() async throws {
        let device = ScopedStubDevice(
            serial: serial, storeID: store1,
            rides: [deviceRide(1, name: "Old era ride", start: 1_700_000_000)])
        let library = InMemoryLibraryStore()
        let model = makeModel(device: device, library: library)
        model.start()
        try await waitFor("identity settles") { model.connectedScope != nil }
        model.sync.sync()
        try await waitFor("old-era ride lands") { library.rideSummaries().count == 1 }

        // A replacement store recycles object id 1 for a new ride.
        device.setIdentity(storeID: store2)
        device.setRides([deviceRide(1, name: "New era ride", start: 1_800_000_000)])
        device.bounce()
        try await waitFor("the new era's scope") {
            model.connectedScope == LibraryScope(serial: serial, storeID: store2)
        }

        model.sync.sync()
        try await waitFor("the new era's ride lands") { library.rideSummaries().count == 2 }

        let oldID = RideID(deviceObjectID: DeviceObjectID(1),
                           scope: LibraryScope(serial: serial, storeID: store1))
        let newID = RideID(deviceObjectID: DeviceObjectID(1),
                           scope: LibraryScope(serial: serial, storeID: store2))
        #expect(Set(library.rideSummaries().map(\.id)) == [oldID, newID],
                "the old era's row is archival; the new era's ride is a distinct row")
        #expect(library.syncedRideIDs() == [oldID, newID])

    }

    /// A tombstone belongs to one store. A replacement store can reuse its
    /// object ID without the old tombstone suppressing that ride.
    @Test func tombstonesDoNotCarryAcrossEras() async throws {
        let device = ScopedStubDevice(
            serial: serial, storeID: store1,
            rides: [deviceRide(5, name: "To be deleted", start: 1_700_000_000)])
        let library = InMemoryLibraryStore()
        let model = makeModel(device: device, library: library)
        model.start()
        try await waitFor("identity settles") { model.connectedScope != nil }
        model.sync.sync()
        try await waitFor("the ride lands") { library.rideSummaries().count == 1 }

        // Phone-side permanent delete (trash → delete forever).
        let oldID = RideID(deviceObjectID: DeviceObjectID(5),
                           scope: LibraryScope(serial: serial, storeID: store1))
        model.deleteRide(oldID)
        model.deleteRideForever(oldID)
        #expect(library.deletedRideIDs() == [oldID])

        // A replacement store contains the same ride under a different StoreId.
        device.setIdentity(storeID: store2)
        device.bounce()
        try await waitFor("the new era's scope") {
            model.connectedScope == LibraryScope(serial: serial, storeID: store2)
        }
        model.sync.sync()
        let newID = RideID(deviceObjectID: DeviceObjectID(5),
                           scope: LibraryScope(serial: serial, storeID: store2))
        try await waitFor("the ride re-syncs under the new era") {
            library.rideSummaries().contains { $0.id == newID }
        }
        #expect(model.rides.map(\.id) == [newID], "visible again — resurrected once, by design")
    }

    /// Missing or failed identity keeps sync closed until a later connection succeeds.
    @Test(arguments: [false, true])
    func unavailableIdentityClosesSyncAndRecovers(readFails: Bool) async throws {
        let device = ScopedStubDevice(
            serial: serial, storeID: store1,
            rides: [deviceRide(1, name: "Unreachable treasure", start: 1_700_000_000)])
        device.setFailIdentityRead(readFails)
        if !readFails { device.setIdentity(storeID: nil) }
        let library = InMemoryLibraryStore()
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(9),
                                      scope: LibraryScope(serial: serial, storeID: store1)))
        let model = makeModel(device: device, library: library)
        model.start()

        // The gate settles closed: a SYNC tap comes straight back to idle.
        model.sync.sync()
        try await waitFor("the vetoed sync returns to idle") {
            model.sync.syncState == .idle && model.sync.syncProgress == nil
        }
        await model.sync.identitySettled()
        #expect(!model.sync.canSync())
        #expect(device.downloadRequests.isEmpty)
        #expect(library.rideSummaries().isEmpty, "nothing synced under an unknown era")
        #expect(model.connectedScope == nil)

        device.setFailIdentityRead(false)
        device.setIdentity(storeID: store1)
        device.bounce()
        try await waitFor("the healed scope") { model.connectedScope != nil }
        model.sync.sync()
        try await waitFor("the ride lands") { library.rideSummaries().count == 1 }
    }

    @Test func connectionPreservesArchivesAndScopesSync() async throws {
        let start: TimeInterval = 1_700_000_000
        let archive = deviceRide(3, name: "Archived", start: start)
        let trashed = deviceRide(5, name: "Trashed archive", start: start)
        let deletedID = RideID(deviceObjectID: DeviceObjectID(4))
        let trashDate = Date(timeIntervalSince1970: floor(Date().timeIntervalSince1970))
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let library = FileLibraryStore(directory: directory)
        try library.saveRide(archive)
        try library.saveRide(trashed)
        library.markRideSynced(archive.id)
        library.markRideDeleted(deletedID)
        library.markRideTrashed(trashed.id, at: trashDate)

        let device = ScopedStubDevice(serial: serial, storeID: store1, rides: [
            archive, deviceRide(4, name: "Current ride", start: start), trashed,
        ])
        let model = makeModel(device: device, library: library)
        model.start()
        try await waitFor("identity settles") { model.connectedScope != nil }
        model.sync.sync()
        try await waitFor("current rides sync") { library.rideSummaries().count == 5 }

        let scope = LibraryScope(serial: serial, storeID: store1)
        let currentIDs = Set([3, 4, 5].map {
            RideID(deviceObjectID: DeviceObjectID($0), scope: scope)
        })
        #expect(library.syncedRideIDs() == currentIDs.union([archive.id]))
        #expect(library.deletedRideIDs() == [deletedID])
        #expect(library.trashedRideIDs() == [trashed.id: trashDate])
        #expect(Set(library.rideSummaries().map(\.id)) == currentIDs.union([archive.id, trashed.id]))
        #expect(library.rideSummaries().first { $0.id == archive.id } == archive.summary)
        #expect(library.ridePoints(archive.id) == archive.points)
        #expect(library.rideSummaries().first { $0.id == trashed.id } == trashed.summary)
        #expect(library.ridePoints(trashed.id) == trashed.points)
    }
}

extension Ride {
    /// The same ride under the id the wire request used: ids live outside the payload, so this is
    /// what a real device does when it serves an object.
    fileprivate func ride(withID id: RideID) -> Ride {
        Ride(summary: RideSummary(
            id: id, name: summary.name, date: summary.date,
            distanceMeters: summary.distanceMeters, movingTime: summary.movingTime,
            averageSpeedMps: summary.averageSpeedMps, climbMeters: summary.climbMeters,
            trackPreview: summary.trackPreview), points: points)
    }
}
