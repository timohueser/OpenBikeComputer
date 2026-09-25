import Foundation
import Testing
import Observation
import OBCDomain
import OBCMock
@testable import OBCTransport
@testable import OBCUI

@Suite("Durable archive confirmation", .timeLimit(.minutes(1)))
@MainActor
struct ArchiveReceiptSyncTests {
    private enum Failure: Error { case write }

    private func directory() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }
    private func corruptPoints(in dir: URL) throws {
        let files = FileManager.default.enumerator(at: dir, includingPropertiesForKeys: nil)!
        for case let file as URL in files where file.lastPathComponent.hasPrefix("points-") {
            try Data("invalid".utf8).write(to: file)
        }
    }
    private func ride(_ object: UInt64 = 41, revision: UInt64 = 1) -> Ride {
        let scope = LibraryScope(serial: "receipts", storeID: String(repeating: "a", count: 32))
        let id = RideID(deviceObjectID: DeviceObjectID(object), scope: scope)
        let date = Date(timeIntervalSince1970: 1_800_000_000)
        var ride = Ride(summary: RideSummary(id: id, name: "Ride \(object)-\(revision)", date: date,
                                             distanceMeters: 10), points: [
            RidePoint(timestamp: date, coordinate: Coordinate(latitude: 48, longitude: 9), segmentStart: true)
        ])
        let payload = RideObjectCodec.encode(ride)
        ride.summary.source = RideSource(storeID: scope.storeID, objectID: object, revision: revision,
                                         payloadLength: UInt64(payload.count), payloadCRC32: CRC32.checksum(payload))
        return ride
    }
    private func setup(_ rides: [Ride], library: any LibraryStore, failures: Int = 0)
        -> (RideSyncCoordinator, ReceiptPeer, MockControl) {
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let peer = ReceiptPeer(rides: rides, failures: failures)
        let transport = ReceiptTransport(base: MockTransport(control: control), peer: peer)
        return (RideSyncCoordinator(transport: transport, library: library,
                                   timing: .init(syncDoneHold: .zero, syncedLineHold: .seconds(300),
                                                 confirmRetryDelay: .zero)), peer, control)
    }
    private func wait(_ condition: () -> Bool) async {
        while !condition() {
            await withCheckedContinuation { continuation in
                withObservationTracking { _ = condition() } onChange: {
                    Task { @MainActor in continuation.resume() }
                }
            }
        }
    }
    private func waitConnection(_ sync: RideSyncCoordinator, _ state: ConnectionState) async {
        // Connection is intentionally excluded from observation in the coordinator.
        while sync.connection != state { await Task.yield() }
    }

    @Test func newArchiveAndReconnectConfirmWithoutDownloadingAgain() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir)
        let ride = ride()
        let source = ride.summary.source!
        let (sync, peer, control) = setup([ride], library: library, failures: 2)
        await waitConnection(sync, .connected)
        sync.sync()
        await wait { sync.syncInterruption?.reason == .confirmationPending }
        #expect(library.archivedRideSource(ride.id) == source)
        #expect(sync.lastSyncCount == nil)
        #expect(await peer.downloads == [[ride.id]])
        #expect(await peer.receipts == [source, source], "one automatic resend before the banner")
        control.connection = .outOfRange
        await waitConnection(sync, .outOfRange)
        control.connection = .connected
        await peer.waitForReceipts(3)
        await wait { sync.syncState == .idle }
        #expect(sync.syncInterruption == nil)
        #expect(await peer.downloads == [[ride.id]])
        #expect(await peer.receipts == [source, source, source])
    }

    @Test func oneUnansweredReceiptIsResentWithoutABanner() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir)
        let ride = ride()
        let (sync, peer, _) = setup([ride], library: library, failures: 1)
        await waitConnection(sync, .connected)
        sync.sync()
        await wait { sync.lastSyncCount == 1 }
        #expect(sync.syncInterruption == nil)
        #expect(await peer.receipts == [ride.summary.source!, ride.summary.source!])
    }

    @Test func reopenedArchiveRevalidatesBeforeNoNewDownloadReturn() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let original = ride()
        _ = try FileLibraryStore(directory: dir).archiveRide(original)
        let library = FileLibraryStore(directory: dir)
        let (sync, peer, _) = setup([original], library: library)
        await peer.waitForReceipts(1)
        await wait { sync.syncState == .idle }
        sync.sync()
        await wait { sync.upToDateToastVisible }
        #expect(await peer.receipts.count == 2)
        #expect(await peer.downloads.isEmpty)
    }

    @Test func partialBatchKeepsFirstArchiveAndRetriesOnlyMissingDownload() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir)
        let first = ride(), second = ride(42)
        let (sync, peer, _) = setup([first, second], library: library)
        await peer.setPartial(true)
        await waitConnection(sync, .connected)
        sync.sync()
        await wait { sync.syncInterruption?.reason == .download }
        #expect(library.archivedRideSource(first.id) == first.summary.source)
        #expect(library.archivedRideSource(second.id) == nil)
        #expect(await peer.receipts == [first.summary.source!])
        await peer.setPartial(false)
        sync.resumeSync()
        await wait { sync.lastSyncCount == 1 }
        #expect(await peer.downloads == [[first.id, second.id], [second.id]])
        #expect(library.archivedRideSource(second.id) == second.summary.source)
    }

    @Test func failedArchiveNeverSendsProof() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir, archiveCheckpoint: { _ in throw Failure.write })
        let (sync, peer, _) = setup([ride()], library: library)
        await waitConnection(sync, .connected)
        sync.sync()
        await wait { sync.syncInterruption != nil }
        #expect(await peer.receipts.isEmpty)
        #expect(library.rideSummaries().isEmpty)
    }

    @Test(arguments: ["missing", "corrupt", "deleted", "trash", "replaced", "other-card"])
    func existingMarkersCannotAuthorizeWrongOrMissingArchive(kind: String) async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir)
        let original = ride()
        _ = try library.archiveRide(original)
        var current = original
        switch kind {
        case "missing":
            try FileManager.default.removeItem(at: dir.appendingPathComponent("rides"))
            library.markRideSynced(original.id)
        case "corrupt":
            try corruptPoints(in: dir)
        case "deleted": library.deleteRide(original.id)
        case "trash": library.markRideTrashed(original.id, at: Date())
        case "replaced": current = ride(revision: 2)
        default:
            let scope = LibraryScope(serial: "receipts", storeID: String(repeating: "b", count: 32))
            current.summary = RideSummary(id: RideID(deviceObjectID: DeviceObjectID(41), scope: scope),
                                         name: "New card", date: original.summary.date, distanceMeters: 10,
                                         source: RideSource(storeID: scope.storeID, objectID: 41, revision: 1,
                                                            payloadLength: 1, payloadCRC32: 0))
        }
        let (sync, peer, _) = setup([current], library: library)
        await waitConnection(sync, .connected)
        await wait { sync.syncState == .idle }
        #expect(await peer.receipts.isEmpty)
        #expect(await peer.downloads.isEmpty, "reconnect does not download new rides")
    }

    @Test func supersededConfirmationCannotChangeTheNewRun() async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir)
        let original = ride()
        let (sync, peer, control) = setup([original], library: library)
        await peer.holdFirstConfirmation()
        await waitConnection(sync, .connected)
        sync.sync()
        await peer.waitForReceipts(1)
        control.connection = .outOfRange
        await wait { sync.syncInterruption != nil }
        control.connection = .connected
        await waitConnection(sync, .connected)
        sync.sync()
        await wait { sync.upToDateToastVisible }
        #expect(await peer.receipts.count == 2)
        await peer.releaseConfirmation()
        for _ in 0..<20 { await Task.yield() }
        #expect(sync.syncState == .idle)
        #expect(sync.syncInterruption == nil)
        #expect(sync.lastSyncCount == nil)
        #expect(await peer.downloads == [[original.id]])
    }

    @Test(arguments: [RideArchiveConfirmation.unsupported, .sourceUnavailable, .refused])
    func terminalRefusalKeepsTheArchiveAndExplainsPendingConfirmation(result: RideArchiveConfirmation) async throws {
        let dir = try directory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let library = FileLibraryStore(directory: dir)
        let original = ride()
        _ = try library.archiveRide(original)
        let (sync, peer, _) = setup([original], library: library)
        await peer.setConfirmation(result)
        await wait { sync.syncInterruption != nil }
        #expect(sync.lastSyncCount == nil)
        #expect(!sync.upToDateToastVisible)
        #expect(library.archivedRideSource(original.id) == original.summary.source)
        #expect(await peer.downloads.isEmpty)
        #expect(await peer.receipts.count == 1)
        let expected: RideSyncCoordinator.SyncInterruption.Reason = switch result {
        case .unsupported: .unsupported
        case .sourceUnavailable: .sourceUnavailable
        default: .refused
        }
        #expect(sync.syncInterruption?.reason == expected)
    }

    @Test func inMemorySaveIsNotDurableProof() async throws {
        let original = ride()
        let library = InMemoryLibraryStore()
        let (sync, peer, control) = setup([original], library: library)
        await waitConnection(sync, .connected)
        sync.sync()
        await wait { sync.lastSyncCount == 1 && sync.syncState == .idle }
        #expect(library.archivedRideSource(original.id) == original.summary.source)
        #expect(library.archivedRideReceipt(original.id) == nil)
        sync.sync()
        await wait { sync.upToDateToastVisible }
        control.connection = .outOfRange
        await waitConnection(sync, .outOfRange)
        control.connection = .connected
        await waitConnection(sync, .connected)
        await wait { sync.syncState == .idle }
        #expect(await peer.receipts.isEmpty)
        #expect(await peer.downloads == [[original.id]])
    }
}

private actor ReceiptPeer {
    let rides: [Ride]
    var failures: Int
    var receipts: [RideSource] = []
    var downloads: [[RideID]] = []
    var partial = false
    var confirmation: RideArchiveConfirmation = .confirmed
    var holdFirst = false
    var held: CheckedContinuation<Void, Never>?
    private var receiptWaiter: (Int, CheckedContinuation<Void, Never>)?
    func waitForReceipts(_ count: Int) async {
        if receipts.count < count {
            await withCheckedContinuation { receiptWaiter = (count, $0) }
        }
    }
    private func received() {
        if let (count, continuation) = receiptWaiter, receipts.count >= count {
            receiptWaiter = nil
            continuation.resume()
        }
    }
    func holdFirstConfirmation() { holdFirst = true }
    func releaseConfirmation() { held?.resume(); held = nil }
    func setConfirmation(_ value: RideArchiveConfirmation) { confirmation = value }
    init(rides: [Ride], failures: Int) { self.rides = rides; self.failures = failures }
    func setPartial(_ value: Bool) { partial = value }
    func catalog() -> RideCatalog { RideCatalog(rides: rides.map(\.summary)) }
    func confirm(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation {
        receipts.append(receipt.source)
        if holdFirst {
            holdFirst = false
            await withCheckedContinuation { held = $0; received() }
        }
        received()
        if failures > 0 { failures -= 1; throw DeviceError.writeFailed }
        return confirmation
    }
    func download(_ ids: [RideID]) -> ([DownloadedRide], Bool) {
        downloads.append(ids)
        let selected = rides.filter { ids.contains($0.id) }
        let returned = partial ? Array(selected.prefix(1)) : selected
        return (returned.map { DownloadedRide(id: $0.id, payload: RideObjectCodec.encode($0), source: $0.summary.source) }, partial)
    }
}

private struct ReceiptTransport: DeviceLink, DeviceObjects {
    let base: MockTransport
    let peer: ReceiptPeer
    var state: AsyncStream<ConnectionState> { base.state }
    func connect() async throws { try await base.connect() }
    func disconnect() async { await base.disconnect() }
    func deviceInfo() async throws -> DeviceInfo { try await base.deviceInfo() }
    func listRoutes() async throws -> [RouteCatalogEntry] { [] }
    func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail { try await base.routeDetail(id) }
    func uploadRoute(_ route: RouteBlob) -> TransferHandle { base.uploadRoute(route) }
    func deleteRoute(_ id: DeviceObjectID) async throws {}
    func listRides() async throws -> RideCatalog { await peer.catalog() }
    func confirmRideArchive(_ receipt: RideArchiveReceipt) async throws -> RideArchiveConfirmation { try await peer.confirm(receipt) }
    func downloadRides(_ ids: [RideID]) -> RideDownload {
        let (stream, continuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        Task {
            let (rides, partial) = await peer.download(ids)
            for ride in rides { continuation.yield(ride) }
            continuation.finish(throwing: partial ? DeviceError.transferDropped : nil)
        }
        return RideDownload(handle: .immediatelyFinished(), rides: stream)
    }
}
