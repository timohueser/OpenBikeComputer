import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Saves each completed ride independently, then confirms its exact durable archive on the
/// device. Reconnect reconciles saved archives; only an explicit sync downloads missing rides.
@MainActor @Observable
public final class RideSyncCoordinator {
    /// Ride-count progress for the syncing caption.
    public struct SyncProgress: Equatable, Sendable {
        public var done: Int
        public var total: Int
    }

    /// A sync the link dropped out from under, which feeds the warning banner and its Resume.
    /// What landed is already persisted; `resumeSync()` continues the rest.
    public struct SyncInterruption: Equatable, Sendable {
        public var landed: Int
        public var total: Int
        public var reason: Reason = .download

        public enum Reason: Equatable, Sendable {
            case download, confirmationPending, unsupported, sourceUnavailable, refused
        }

        public var title: String {
            reason == .download ? "Sync interrupted." : "Device confirmation pending."
        }
        public var message: String {
            switch reason {
            case .download: "Got \(landed) of \(total) rides."
            case .confirmationPending: "Your rides are saved on this phone. Retry to confirm them on the device."
            case .unsupported: "Your rides are saved on this phone. This device does not support archive confirmation."
            case .sourceUnavailable: "Your rides are saved on this phone. The device ride changed or is no longer available."
            case .refused: "Your rides are saved on this phone. The device refused archive confirmation."
            }
        }
    }

    /// Pacing, injectable so the coordinator tests run in milliseconds.
    public struct Timing: Sendable {
    /// How long the forest check holds before the button returns to idle.
        public var syncDoneHold: Duration
    /// How long the "synced N new rides just now" line stays up.
        public var syncedLineHold: Duration

        public init(
            syncDoneHold: Duration = .seconds(2),
            syncedLineHold: Duration = .seconds(60)
        ) {
            self.syncDoneHold = syncDoneHold
            self.syncedLineHold = syncedLineHold
        }
    }

    // MARK: Observable state

    public private(set) var syncState: OBCSyncButtonState = .idle {
        // The batch is mid-flight exactly while `.syncing`: `runSync` raises this, and every way
        // out lowers it. Mirroring the transitions here keeps the `TransferActivity` claim in
        // lock-step with the state machine's many exits, including an interruption whose consuming
        // loop deliberately stays alive awaiting the stalled stream. A stalled batch must not hold
        // the background grace window: waiting longer will not finish a transfer whose link is
        // gone, and Resume restarts it after the foreground reconnect.
        didSet {
            guard oldValue != syncState else { return }
            if syncState == .syncing {
                if activityToken == nil { activityToken = activity?.begin() }
            } else if let token = activityToken {
                activityToken = nil
                activity?.end(token)
            }
        }
    }
    /// Non-nil while syncing, which feeds the amber progress caption.
    public private(set) var syncProgress: SyncProgress?
    /// Non-nil after a successful sync, which feeds the confirm line.
    public private(set) var lastSyncCount: Int?
    /// A sync found nothing new; bound to the transient toast.
    public var upToDateToastVisible = false
    /// Non-nil while a dropped sync waits for Resume. It replaces the disconnected banner, because
    /// one banner at a time, and this one carries the link story too.
    public private(set) var syncInterruption: SyncInterruption?
    /// How many rides the device holds beyond what its bounded catalog could carry, set from each
    /// sync's list read. Above zero it surfaces the "some rides can't be listed" warning: past the
    /// device's cap the catalog scan drops the excess in arbitrary order, so this is the only
    /// honest "you are not actually up to date" signal. It holds within a connected session, but
    /// resets on every edge into `.connected`: a count carried across a link edge could be stale,
    /// or another device's entirely, and the banner names the connected device.
    public private(set) var hiddenRideCount: Int = 0

    // MARK: Wiring

    private let transport: any DeviceLink & DeviceObjects
    private let library: any LibraryStore
    private let timing: Timing
    /// The foreground-only policy's in-flight ledger. Nil in tests and previews.
    @ObservationIgnored private let activity: TransferActivity?
    /// This coordinator's claim while a batch is syncing.
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    /// The model's veto: false while the connected device reports an incompatible protocol
    /// version, and also while the verdict is still unknown, so an id-keyed operation cannot run
    /// ahead of the check. The coordinator only asks, and always after awaiting `identitySettled`.
    @ObservationIgnored public var canSync: () -> Bool = { true }
    /// Awaits the model's identity read settling for the current connection. `runSync` suspends on
    /// this before consulting `canSync`, so an early tap waits the few milliseconds for the verdict
    /// instead of silently doing nothing. Defaults to already-settled.
    @ObservationIgnored public var identitySettled: () async -> Void = {}
    /// A ride just landed and was persisted, so the model can mirror it into its in-memory list.
    /// Delivery is per ride, so newly synced rides surface this session.
    @ObservationIgnored public var onRideLanded: (Ride) -> Void = { _ in }
    /// The batch's `listRides()` read succeeded, which proves the device is readable, so the model
    /// can clear a stale failure state.
    @ObservationIgnored public var onRideCatalogRead: () -> Void = {}
    /// What makes the next sync's "new". Re-read from the library at the start of every sync,
    /// because the store is the source of truth: a phone-side tombstone reaches the coordinator
    /// through that re-read, with no cross-object mirror pokes.
    @ObservationIgnored private var syncedRideIDs: Set<RideID> = []
    /// The coordinator's own view of the link. It gates `sync()` and is kept by its own state
    /// subscription, which replays on subscribe.
    @ObservationIgnored private(set) var connection: ConnectionState = .connecting
    @ObservationIgnored private var connectionWatch: Task<Void, Never>?
    @ObservationIgnored private var syncTask: Task<Void, Never>?
    @ObservationIgnored private var syncDropWatch: Task<Void, Never>?
    /// The running, or dropped but resumable, download. `resumeSync()` signals its handle, and the
    /// consuming loop in `runSync` is still awaiting its stream.
    @ObservationIgnored private var activeDownload: RideDownload?

    public init(
        transport: any DeviceLink & DeviceObjects,
        library: any LibraryStore,
        timing: Timing = Timing(),
        activity: TransferActivity? = nil
    ) {
        self.transport = transport
        self.library = library
        self.timing = timing
        self.activity = activity
        // The stream never finishes, so a strong capture would pin the coordinator, and its owner,
        // for the session.
        connectionWatch = Task { [weak self, transport] in
            var wasConnected = false
            for await state in transport.state {
                guard let self else { return }
                connection = state
                if state == .connected, !wasConnected {
                    // A fresh link is a fresh device truth, so drop the previous session's
                    // truncation count: it may be stale, or a different device's. The next sync's
                    // list read re-establishes it.
                    hiddenRideCount = 0
                    reconcileArchives()
                }
                wasConnected = state == .connected
            }
        }
    }

    deinit {
        connectionWatch?.cancel()
        syncTask?.cancel()
        syncDropWatch?.cancel()
    }

    /// Explicit sync downloads missing rides and retries device archive confirmation.
    public func sync() {
        guard connection == .connected, syncState != .syncing else { return }
        startSync(downloadMissing: true)
    }

    private func reconcileArchives() {
        guard syncState != .syncing, activeDownload == nil,
              library.archivedRideSummaries().contains(where: { $0.source != nil }) else { return }
        startSync(downloadMissing: false)
    }

    private func startSync(downloadMissing: Bool) {
        syncTask?.cancel()
        syncDropWatch?.cancel()
        activeDownload?.handle.cancel()
        syncInterruption = nil
        activeDownload = nil
        upToDateToastVisible = false
        syncState = .syncing
        syncTask = Task { await runSync(downloadMissing: downloadMissing) }
    }

    /// Resume restarts the dropped batch at whole-ride granularity: rides that fully landed stay
    /// landed, and the interrupted one is re-sent from its start. The consuming loop never stopped,
    /// because it is awaiting the stalled stream, so rides simply start landing again.
    public func resumeSync() {
        guard let interruption = syncInterruption else { return }
        guard let download = activeDownload, interruption.reason == .download else {
            sync()
            return
        }
        syncInterruption = nil
        syncState = .syncing
        syncProgress = SyncProgress(done: interruption.landed, total: interruption.total)
        download.handle.resume()
    }

    private func runSync(downloadMissing: Bool) async {
        defer { if !Task.isCancelled { syncTask = nil } }
        lastSyncCount = nil
        await identitySettled()
        guard !Task.isCancelled else { return }
        guard canSync() else {
            syncState = .idle
            return
        }
        syncedRideIDs = library.syncedRideIDs()
        do {
            let catalog = try await transport.listRides()
            try Task.checkCancellation()
            onRideCatalogRead()
            hiddenRideCount = catalog.hiddenRideCount
            let deleted = library.deletedRideIDs().union(library.trashedRideIDs().keys)
            var fresh: [RideSummary] = []
            var receipts: [RideArchiveReceipt] = []
            for summary in catalog.rides where !deleted.contains(summary.id) {
                if let source = summary.source {
                    if let receipt = library.archivedRideReceipt(summary.id), receipt.source == source {
                        receipts.append(receipt)
                    } else if downloadMissing && library.archivedRideSource(summary.id) != source {
                        fresh.append(summary)
                    }
                } else if downloadMissing && !syncedRideIDs.contains(summary.id) {
                    fresh.append(summary)
                }
            }

            var confirmationFailure: SyncInterruption.Reason?
            for receipt in receipts {
                if let failure = try await confirm(receipt) { confirmationFailure = failure }
            }
            try Task.checkCancellation()
            guard !fresh.isEmpty else {
                syncState = .idle
                if let confirmationFailure {
                    syncInterruption = SyncInterruption(landed: 0, total: 0, reason: confirmationFailure)
                } else if downloadMissing {
                    upToDateToastVisible = true
                }
                return
            }

            syncProgress = SyncProgress(done: 0, total: fresh.count)
            let download = transport.downloadRides(from: fresh)
            activeDownload = download
            let dropWatch = Task { [weak self, transport] in
                for await state in transport.state
                where state == .outOfRange || state == .disconnected {
                    guard let self, !Task.isCancelled else { return }
                    interruptSync()
                }
            }
            syncDropWatch = dropWatch
            defer { dropWatch.cancel() }
            var landed = 0
            var batchFailed = false
            do {
                for try await downloaded in download.rides {
                    try Task.checkCancellation()
                    guard let summary = fresh.first(where: { $0.id == downloaded.id }),
                          downloaded.source == summary.source else { throw DeviceError.readFailed }
                    var ride = try RideObjectCodec.decode(downloaded.payload, id: downloaded.id)
                    if let source = downloaded.source {
                        guard source.matches(downloaded.id),
                              source.payloadLength == UInt64(downloaded.payload.count),
                              source.payloadCRC32 == CRC32.checksum(downloaded.payload) else {
                            throw DeviceError.crcMismatch
                        }
                        ride.summary.source = source
                    } else {
                        let decoded = ride.summary
                        ride.summary = summary
                        if ride.summary.trackPreview == nil { ride.summary.trackPreview = decoded.trackPreview }
                        ride.summary.descentMeters = decoded.descentMeters
                        ride.summary.avgHeartRate = decoded.avgHeartRate
                        ride.summary.maxHeartRate = decoded.maxHeartRate
                        ride.summary.avgCadence = decoded.avgCadence
                        ride.summary.avgPower = decoded.avgPower
                        ride.summary.maxPower = decoded.maxPower
                        ride.summary.energyKJ = decoded.energyKJ
                    }
                    let receipt = try library.archiveRide(ride)
                    syncedRideIDs.insert(downloaded.id)
                    onRideLanded(ride)
                    landed += 1
                    syncProgress = SyncProgress(done: landed, total: fresh.count)
                    if let receipt, let failure = try await confirm(receipt) {
                        confirmationFailure = failure
                    }
                }
            } catch {
                batchFailed = true
                download.handle.cancel()
            }
            try Task.checkCancellation()
            let outcome = await download.handle.outcome
            try Task.checkCancellation()
            syncProgress = nil
            syncInterruption = nil
            activeDownload = nil
            if batchFailed || outcome != .completed || confirmationFailure != nil {
                syncState = .idle
                syncInterruption = SyncInterruption(
                    landed: landed, total: fresh.count,
                    reason: batchFailed || outcome != .completed ? .download : confirmationFailure!)
                return
            }

            lastSyncCount = landed
            syncState = .done
            try? await Task.sleep(for: timing.syncDoneHold)
            guard !Task.isCancelled else { return }
            syncState = .idle
            try? await Task.sleep(for: timing.syncedLineHold)
            guard !Task.isCancelled else { return }
            lastSyncCount = nil
        } catch {
            guard !Task.isCancelled else { return }
            syncProgress = nil
            syncState = .idle
            // A failed reconnect catalog cannot establish which archived sources still match.
            syncInterruption = SyncInterruption(
                landed: 0, total: 0, reason: downloadMissing ? .download : .confirmationPending)
        }
    }

    private func confirm(_ receipt: RideArchiveReceipt) async throws -> SyncInterruption.Reason? {
        try Task.checkCancellation()
        do {
            let result = try await transport.confirmRideArchive(receipt)
            try Task.checkCancellation()
            switch result {
            case .confirmed: return nil
            case .sourceUnavailable: return .sourceUnavailable
            case .unsupported: return .unsupported
            case .refused: return .refused
            }
        } catch {
            try Task.checkCancellation()
            return .confirmationPending
        }
    }

    /// The drop watch's hand-off: freeze the counts into the banner state and bring the progress
    /// caption down. The download stays resumable.
    private func interruptSync() {
        guard syncState == .syncing, activeDownload != nil else { return }
        syncState = .idle
        syncInterruption = SyncInterruption(
            landed: syncProgress?.done ?? 0,
            total: syncProgress?.total ?? 0
        )
        syncProgress = nil
    }
}
