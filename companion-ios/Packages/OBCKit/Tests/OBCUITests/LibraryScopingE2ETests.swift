import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// End-to-end (serial, epoch, id) keying (#769): a stub device that mints
/// scoped catalog ids exactly like `BLETransport` (from its own identity read),
/// driven through the real `MainScreenModel` + `RideSyncCoordinator` — a full
/// mock sync lands rides under composite keys, an era change replays the
/// 2026-07-12 incident self-healingly, and the claim migration runs on the
/// model's own connect flow.
@MainActor @Suite struct LibraryScopingE2ETests {
    // MARK: The stub device

    /// A device with a mutable identity (serial, epoch) and ride store; its
    /// `listRides()` mints ids scoped to the identity the *last `deviceInfo()`
    /// read* returned — the same order-of-truth as the real transport.
    final class ScopedStubDevice: DeviceTransport, @unchecked Sendable {
        private let stateMulticast = AsyncMulticast<ConnectionState>(.connected)
        private let lock = NSLock()
        private var _serial: String
        private var _epoch: UInt32?
        private var _rides: [Ride]
        private var _acked: [[RideID]] = []
        private var _failIdentityRead = false

        init(serial: String, epoch: UInt32?, rides: [Ride] = []) {
            _serial = serial
            _epoch = epoch
            _rides = rides
        }

        // Test knobs.
        func setIdentity(serial: String? = nil, epoch: UInt32?) {
            lock.withLock {
                if let serial { _serial = serial }
                _epoch = epoch
            }
        }
        func setRides(_ rides: [Ride]) { lock.withLock { _rides = rides } }
        func setFailIdentityRead(_ fail: Bool) { lock.withLock { _failIdentityRead = fail } }
        var ackedBatches: [[RideID]] { lock.withLock { _acked } }
        var scope: LibraryScope? {
            lock.withLock { _epoch.map { LibraryScope(serial: _serial, epoch: $0) } }
        }
        func bounce() {
            stateMulticast.send(.disconnected)
            stateMulticast.send(.connected)
        }

        // DeviceTransport.
        var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
        var battery: AsyncStream<Int> { AsyncStream { $0.finish() } }
        var storeChanges: AsyncStream<StoreChanged> { AsyncStream { $0.finish() } }
        func connect() async throws {}
        func disconnect() async {}

        func deviceInfo() async throws -> DeviceInfo {
            let (serial, epoch, fail) = lock.withLock { (_serial, _epoch, _failIdentityRead) }
            if fail { throw DeviceError.readFailed }
            return DeviceInfo(name: "Trailhead", firmwareVersion: "2.0", serial: serial,
                              storeEpoch: epoch)
        }

        func readConfig() async throws -> DeviceConfig { DeviceConfig(name: "Trailhead") }
        func writeConfig(_ config: DeviceConfig) async throws {}
        func listRoutes() async throws -> [RouteCatalogEntry] { [] }
        func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
            throw DeviceError.readFailed
        }
        func uploadRoute(_ route: RouteBlob) -> TransferHandle {
            .immediatelyFinished(.failed(.notConnected))
        }
        func deleteRoute(_ id: DeviceObjectID) async throws {}
        func rideDetail(_ id: RideID) async throws -> RideDetail { throw DeviceError.readFailed }
        func readDiagnostics() async throws -> Data { Data() }

        /// Scoped minting, like `BLETransport.listRides()` (#769).
        func listRides() async throws -> RideCatalog {
            let (rides, scope) = lock.withLock {
                (_rides, _epoch.map { LibraryScope(serial: _serial, epoch: $0) })
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
            let rides = lock.withLock { _rides }
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

        func ackRides(_ ids: [RideID]) async throws {
            guard !ids.isEmpty else { return }
            lock.withLock { _acked.append(ids) }
        }
    }

    // MARK: Helpers

    private let epoch1: UInt32 = 0x1111_1111
    private let epoch2: UInt32 = 0x2222_2222
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

    private func waitFor(
        _ what: String, timeout: Duration = .seconds(30), _ condition: () -> Bool
    ) async {
        let deadline = ContinuousClock.now.advanced(by: timeout)
        while !condition() {
            if ContinuousClock.now > deadline {
                Issue.record("timed out waiting for \(what)")
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
    }

    private func makeModel(
        device: ScopedStubDevice, library: any LibraryStore
    ) -> MainScreenModel {
        MainScreenModel(
            transport: device, library: library,
            syncTiming: .init(syncDoneHold: .seconds(300), syncedLineHold: .seconds(300)))
    }

    // MARK: End-to-end composite keying

    /// A full sync against a scoped-minting device lands every ride — entry,
    /// synced mark — under (serial, epoch, id) keys, and the next connect's
    /// possession ack sends exactly those scoped ids.
    @Test func syncLandsRidesUnderCompositeKeys() async {
        let device = ScopedStubDevice(
            serial: serial, epoch: epoch1,
            rides: [deviceRide(1, name: "Dawn Patrol", start: 1_700_000_000),
                    deviceRide(2, name: "Gravel Hour", start: 1_700_100_000)])
        let library = InMemoryLibraryStore()
        let model = makeModel(device: device, library: library)
        model.start()
        await waitFor("identity settles") { model.connectedScope != nil }

        model.sync.sync()
        await waitFor("both rides land") { library.rideSummaries().count == 2 }

        let scope = LibraryScope(serial: serial, epoch: epoch1)
        let expected: Set<RideID> = [
            RideID(deviceObjectID: DeviceObjectID(1), scope: scope),
            RideID(deviceObjectID: DeviceObjectID(2), scope: scope),
        ]
        #expect(Set(library.rideSummaries().map(\.id)) == expected)
        #expect(library.syncedRideIDs() == expected)
        // The rows carry their content, not just their keys.
        #expect(Set(library.rideSummaries().map(\.name)) == ["Dawn Patrol", "Gravel Hour"])
        #expect(library.ridePoints(RideID(deviceObjectID: DeviceObjectID(1), scope: scope))?.isEmpty == false)

        // A reconnect acks exactly the scoped possession list.
        device.bounce()
        await waitFor("the reconnect ack") { !device.ackedBatches.isEmpty }
        #expect(Set(device.ackedBatches.last ?? []) == expected)
    }

    /// The 2026-07-12 incident, replayed self-healing: the device is wiped
    /// (fresh epoch, fresh rides under recycled object ids). The old synced
    /// set must not filter the new rides ("sync forever answers up to date"),
    /// the old rows must survive as archival entries, and no ack may stamp
    /// old ids onto the new era.
    @Test func eraChangeSyncsTheNewErasRidesAndKeepsTheOldArchival() async {
        let device = ScopedStubDevice(
            serial: serial, epoch: epoch1,
            rides: [deviceRide(1, name: "Old era ride", start: 1_700_000_000)])
        let library = InMemoryLibraryStore()
        let model = makeModel(device: device, library: library)
        model.start()
        await waitFor("identity settles") { model.connectedScope != nil }
        model.sync.sync()
        await waitFor("old-era ride lands") { library.rideSummaries().count == 1 }

        // Chip-erase: new epoch, a NEW ride recycles object id 1.
        device.setIdentity(epoch: epoch2)
        device.setRides([deviceRide(1, name: "New era ride", start: 1_800_000_000)])
        device.bounce()
        await waitFor("the new era's scope") {
            model.connectedScope == LibraryScope(serial: serial, epoch: epoch2)
        }

        model.sync.sync()
        await waitFor("the new era's ride lands") { library.rideSummaries().count == 2 }

        let oldID = RideID(deviceObjectID: DeviceObjectID(1),
                           scope: LibraryScope(serial: serial, epoch: epoch1))
        let newID = RideID(deviceObjectID: DeviceObjectID(1),
                           scope: LibraryScope(serial: serial, epoch: epoch2))
        #expect(Set(library.rideSummaries().map(\.id)) == [oldID, newID],
                "the old era's row is archival; the new era's ride is a distinct row")
        #expect(library.syncedRideIDs() == [oldID, newID])
        // No ack ever carried an old-era id after the era change.
        let postEraAcks = device.ackedBatches.filter { $0.contains(where: { $0.scope?.epoch == epoch1 }) }
        #expect(postEraAcks.allSatisfy { batch in batch.allSatisfy { $0.scope?.epoch == epoch1 } })
    }

    /// A phone-side tombstone dies with its era: after the wipe the (kept
    /// card's) ride under a matching object id re-syncs once — resurrection
    /// is the accepted safe direction, silent suppression the incident.
    @Test func tombstonesDoNotCarryAcrossEras() async {
        let device = ScopedStubDevice(
            serial: serial, epoch: epoch1,
            rides: [deviceRide(5, name: "To be deleted", start: 1_700_000_000)])
        let library = InMemoryLibraryStore()
        let model = makeModel(device: device, library: library)
        model.start()
        await waitFor("identity settles") { model.connectedScope != nil }
        model.sync.sync()
        await waitFor("the ride lands") { library.rideSummaries().count == 1 }

        // Phone-side permanent delete (trash → delete forever).
        let oldID = RideID(deviceObjectID: DeviceObjectID(5),
                           scope: LibraryScope(serial: serial, epoch: epoch1))
        model.deleteRide(oldID)
        model.deleteRideForever(oldID)
        #expect(library.deletedRideIDs() == [oldID])

        // RRAM-only wipe: new epoch, the card kept the ride.
        device.setIdentity(epoch: epoch2)
        device.bounce()
        await waitFor("the new era's scope") {
            model.connectedScope == LibraryScope(serial: serial, epoch: epoch2)
        }
        model.sync.sync()
        let newID = RideID(deviceObjectID: DeviceObjectID(5),
                           scope: LibraryScope(serial: serial, epoch: epoch2))
        await waitFor("the ride re-syncs under the new era") {
            library.rideSummaries().contains { $0.id == newID }
        }
        #expect(model.rides.map(\.id) == [newID], "visible again — resurrected once, by design")
    }

    /// Ack fail-closed, end to end: a connection whose identity read throws
    /// sends no ack and syncs nothing — and the next good connection heals.
    @Test func failedIdentityReadClosesAckAndSync() async {
        let device = ScopedStubDevice(
            serial: serial, epoch: epoch1,
            rides: [deviceRide(1, name: "Unreachable treasure", start: 1_700_000_000)])
        device.setFailIdentityRead(true)
        let library = InMemoryLibraryStore()
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(9),
                                      scope: LibraryScope(serial: serial, epoch: epoch1)))
        let model = makeModel(device: device, library: library)
        model.start()

        // The gate settles closed: a SYNC tap comes straight back to idle.
        model.sync.sync()
        await waitFor("the vetoed sync returns to idle") {
            model.sync.syncState == .idle && model.sync.syncProgress == nil
        }
        try? await Task.sleep(for: .milliseconds(100))
        #expect(device.ackedBatches.isEmpty, "no possession ack under an unknown era")
        #expect(library.rideSummaries().isEmpty, "nothing synced under an unknown era")
        #expect(model.connectedScope == nil)

        // The next connection reads identity fine → ack + sync work.
        device.setFailIdentityRead(false)
        device.bounce()
        await waitFor("the healed scope") { model.connectedScope != nil }
        await waitFor("the ack") { !device.ackedBatches.isEmpty }
        model.sync.sync()
        await waitFor("the ride lands") { library.rideSummaries().count == 1 }
    }

    /// The claim migration runs on the model's own connect flow: a flat v1
    /// library entry the device corroborates is re-keyed before the first
    /// sync's freshness filter runs — one row, never a re-download duplicate.
    @Test func connectFlowClaimsLegacyEntriesBeforeSyncing() async {
        let start: TimeInterval = 1_700_000_000
        let device = ScopedStubDevice(
            serial: serial, epoch: epoch1,
            rides: [deviceRide(3, name: "Corroborated", start: start)])
        let library = InMemoryLibraryStore()
        // The v1 library: the same ride under its flat key.
        library.saveRide(Ride(
            summary: RideSummary(
                id: RideID(deviceObjectID: DeviceObjectID(3)), name: "Corroborated",
                date: Date(timeIntervalSince1970: start), distanceMeters: 20_000),
            points: []))
        library.markRideSynced(RideID(deviceObjectID: DeviceObjectID(3)))

        let model = makeModel(device: device, library: library)
        model.start()
        await waitFor("identity settles") { model.connectedScope != nil }

        let scopedID = RideID(deviceObjectID: DeviceObjectID(3),
                              scope: LibraryScope(serial: serial, epoch: epoch1))
        await waitFor("the claim") { library.rideSummaries().map(\.id) == [scopedID] }
        // The claimed id is what the connect ack sends…
        await waitFor("the ack") { !device.ackedBatches.isEmpty }
        #expect(device.ackedBatches.last == [scopedID])
        // …and the first sync answers up to date instead of duplicating.
        model.sync.sync()
        await waitFor("up to date") { model.sync.upToDateToastVisible }
        #expect(library.rideSummaries().count == 1, "no duplicate row after the claim")
        // The model's own list shows the claimed row.
        #expect(model.rides.map(\.id) == [scopedID])
    }
}

extension Ride {
    /// The same ride under the id the wire request used — what a real device
    /// does when it serves an object (ids live outside the payload).
    fileprivate func ride(withID id: RideID) -> Ride {
        Ride(summary: RideSummary(
            id: id, name: summary.name, date: summary.date,
            distanceMeters: summary.distanceMeters, movingTime: summary.movingTime,
            averageSpeedMps: summary.averageSpeedMps, climbMeters: summary.climbMeters,
            trackPreview: summary.trackPreview), points: points)
    }
}
