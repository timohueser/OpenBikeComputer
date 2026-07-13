import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The main screen's ride-sync state machine (B7), extracted from
/// `MainScreenModel` (#358). Depends only on `DeviceTransport` (the golden
/// rule) plus the `LibraryStore` it persists into.
///
/// **The SYNC button contract:** idle → syncing ("N of M rides") → done
/// ("Synced N new rides just now", ~2 s check) → idle — driven off the
/// `downloadRides` `RideDownload`. "New" means not in the `LibraryStore`'s
/// synced set (B1S) — persistent, so a relaunch never re-counts. Each landed
/// payload decodes through `RideObjectCodec` into the canonical `Ride`
/// and persists at once, so a drop mid-batch keeps what arrived (H10) by
/// construction. A drop surfaces as `syncInterruption` ("Got 2 of 5 rides." +
/// Resume); `resumeSync()` restarts the stalled batch at **whole-ride
/// granularity** — rides that fully landed stay, the rest are re-sent whole
/// (transfers restart, not resume — the spec's principle 4).
///
/// **Division of labor with the model:** persistence happens *here*, per
/// landed ride — `library.saveRide` + `markRideSynced` the moment the bytes
/// arrive is exactly what makes a dropped batch keep its partial. The owning
/// model only mirrors each landed ride into its in-memory list via the
/// `onRideLanded` callback (a plain closure — an `AsyncStream` seam would add
/// ordering questions for no benefit on one actor). The #303 protocol-version
/// gate stays with the model (it belongs to the reload/identity path) and is
/// injected as `canSync`; the link gate lives here, off the coordinator's own
/// `transport.state` subscription.
@MainActor @Observable
public final class RideSyncCoordinator {
    /// Ride-count progress for the syncing caption ("3 of 5 rides").
    public struct SyncProgress: Equatable, Sendable {
        public var done: Int
        public var total: Int
    }

    /// H10 — a sync the link dropped out from under. Feeds the warning banner
    /// ("Sync interrupted. Got 2 of 5 rides." + Resume). What landed is already
    /// persisted; `resumeSync()` continues the rest.
    public struct SyncInterruption: Equatable, Sendable {
        public var landed: Int
        public var total: Int
    }

    /// Pacing — injectable so the coordinator tests run in milliseconds.
    public struct Timing: Sendable {
        /// How long the forest check holds before the button returns to idle
        /// (design: "Check for ~2s, then idle").
        public var syncDoneHold: Duration
        /// How long the C2 "Synced N new rides just now" line stays up.
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
        // The batch is mid-flight exactly while `.syncing` (#459): `runSync`
        // raises it, and every way out — done, idle, the H10 interrupt — lowers
        // it. Mirroring the transitions here keeps the `TransferActivity` claim
        // in lock-step with the state machine's many exit paths, including an
        // interruption whose consuming loop deliberately stays alive awaiting
        // the stalled stream (a stalled batch must NOT hold the background
        // grace window — waiting longer won't finish a transfer whose link is
        // gone; Resume restarts it after the foreground reconnect).
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
    /// Non-nil while syncing — feeds the amber "N of M rides" caption.
    public private(set) var syncProgress: SyncProgress?
    /// Non-nil after a successful sync — feeds "Synced N new rides just now".
    public private(set) var lastSyncCount: Int?
    /// H9: a sync found nothing new (bound to the transient toast).
    public var upToDateToastVisible = false
    /// H10: non-nil while a dropped sync waits for Resume — replaces the S4
    /// banner (one banner at a time; this one carries the link story too).
    public private(set) var syncInterruption: SyncInterruption?
    /// How many rides the device holds beyond what its `rideList` could carry
    /// (v2 header `total − count`, spec §7.4) — set from each sync's list read.
    /// `> 0` surfaces the "some rides can't be listed" warning: past the device's
    /// `MAX_RIDES` cap the catalog scan drops the excess in FAT-arbitrary order,
    /// so this is the only honest "you're not actually up to date" signal. Holds
    /// within a connected session (device truth, re-read every sync) but **resets
    /// on every edge into `.connected`**: a count carried across a link edge could
    /// be stale (the rider freed space while away) or another device's entirely
    /// (the banner interpolates the connected device's name — device A's count
    /// under device B's name would assert a false fact). Unknown-until-read is
    /// the honest state, matching the epic's proof-only philosophy.
    public private(set) var hiddenRideCount: Int = 0

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let library: any LibraryStore
    private let timing: Timing
    /// The foreground-only policy's in-flight ledger (#459) — `nil` in tests
    /// and previews that don't exercise the lifecycle.
    @ObservationIgnored private let activity: TransferActivity?
    /// This coordinator's claim while a batch is `.syncing`.
    @ObservationIgnored private var activityToken: TransferActivity.Token?
    /// The model's veto (#303): `false` while the connected device reports an
    /// incompatible `protocol_version` — that state lives with the model's
    /// reload/identity path; the coordinator only asks.
    @ObservationIgnored public var canSync: () -> Bool = { true }
    /// A ride just landed *and persisted* — the model mirrors it into its
    /// in-memory Tracked list. Delivery is per ride, so newly synced rides
    /// surface this session, not only after the next reload.
    @ObservationIgnored public var onRideLanded: (Ride) -> Void = { _ in }
    /// The batch's `listRides()` read succeeded — proof the device is readable,
    /// so the model can clear a stale S3 failure state.
    @ObservationIgnored public var onRideListRead: () -> Void = {}
    /// Mirror of `library.syncedRideIDs()` — what makes the next sync's "new".
    /// **Re-read from the library at the start of every sync** (the store is
    /// the source of truth): phone-side tombstones (`deleteRide` marks the id
    /// synced so a later sync can't resurrect it) reach the coordinator through
    /// that re-read — no cross-object mirror pokes.
    @ObservationIgnored private var syncedRideIDs: Set<RideID> = []
    /// The coordinator's own view of the link — gates `sync()` and is kept by
    /// its own `transport.state` subscription (replayed on subscribe).
    @ObservationIgnored private(set) var connection: ConnectionState = .connecting
    @ObservationIgnored private var connectionWatch: Task<Void, Never>?
    @ObservationIgnored private var syncTask: Task<Void, Never>?
    @ObservationIgnored private var syncDropWatch: Task<Void, Never>?
    /// The running (or dropped-but-resumable) download — `resumeSync()` signals
    /// its handle; the consuming loop in `runSync` is still awaiting its stream.
    @ObservationIgnored private var activeDownload: RideDownload?

    public init(
        transport: any DeviceTransport,
        library: any LibraryStore,
        timing: Timing = Timing(),
        activity: TransferActivity? = nil
    ) {
        self.transport = transport
        self.library = library
        self.timing = timing
        self.activity = activity
        // Open-ended stream loops are `[weak self]` + per-iteration `guard let
        // self` (the #356 convention) — the stream never finishes, so a strong
        // capture would pin the coordinator (and its owner) for the session.
        connectionWatch = Task { [weak self, transport] in
            var wasConnected = false
            for await state in transport.state {
                guard let self else { return }
                connection = state
                // Possession-ack reconciliation (spec §4.4 `ackRides`), on every
                // edge into `.connected` — including the stream's replayed first
                // value (an app launched against a live link should heal too):
                // send the device the ride ids the library holds, so its
                // per-ride "synced" flag trues up against the phone's ground
                // truth. This is what heals rides synced before the device
                // tracked the flag, a sidecar lost with a reflashed card, or an
                // app reinstall — cases a download-completion event can never
                // reach, because an already-held ride is never re-downloaded.
                if state == .connected, !wasConnected {
                    // A fresh link is a fresh device truth: drop the previous
                    // session's truncation count (see `hiddenRideCount` — it may
                    // be stale, or a *different* device's). The next sync's list
                    // read re-establishes it.
                    hiddenRideCount = 0
                    ackPossessedRides()
                }
                wasConnected = state == .connected
            }
        }
    }

    /// Fire-and-forget the possession ack (the `DeviceNameReconciler` pattern:
    /// a failed send self-heals on the next connect by construction — the whole
    /// list is re-sent every time — so the error is deliberately dropped rather
    /// than surfaced). Captures only the transport and the id snapshot, never
    /// `self`. Tombstoned/trashed rides stay in `syncedRideIDs()` (they landed
    /// once), which is exactly the flag's meaning — "downloaded at least once".
    private func ackPossessedRides() {
        guard canSync() else { return }
        let ids = Array(library.syncedRideIDs())
        guard !ids.isEmpty else { return }
        Task { [transport] in
            try? await transport.ackRides(ids)
        }
    }

    deinit {
        connectionWatch?.cancel()
        syncTask?.cancel()
        syncDropWatch?.cancel()
    }

    // MARK: Sync (the SYNC button)

    /// Pull new tracked rides off the device. No-ops unless the link is up and
    /// no sync is running (the button is disabled when unreachable — S4 dims
    /// link-bound actions). Starting fresh over a waiting interruption is fine:
    /// what landed is marked synced, so the new batch is exactly the remainder.
    public func sync() {
        // `canSync` is the model's #303 veto: on an incompatible device the
        // banner explains why, and decoding its ride objects would be the exact
        // "silently proceed" the version check exists to prevent.
        guard connection == .connected, syncState != .syncing, canSync() else { return }
        syncTask?.cancel()
        syncDropWatch?.cancel()
        // Tear down a superseded (interrupted-but-waiting) batch so its runner
        // stops competing for the transfer slot and its stalled stream finishes —
        // the cancelled old `runSync` then can't yield a late ride into the new
        // sync's shared state.
        activeDownload?.handle.cancel()
        syncInterruption = nil
        activeDownload = nil
        syncTask = Task { await runSync() }
    }

    /// H10's Resume: restart the dropped batch at whole-ride granularity —
    /// rides that fully landed stay landed, the interrupted one is re-sent from
    /// its start. The consuming loop never stopped (it's awaiting the stalled
    /// stream), so rides simply start landing again.
    public func resumeSync() {
        guard let interruption = syncInterruption, let download = activeDownload else { return }
        syncInterruption = nil
        syncState = .syncing
        syncProgress = SyncProgress(done: interruption.landed, total: interruption.total)
        download.handle.resume()
    }

    private func runSync() async {
        syncState = .syncing
        lastSyncCount = nil
        // The mirror trues up per sync (see its doc): a ride synced by an
        // earlier batch — or tombstoned by a phone-side delete — is not "new".
        syncedRideIDs = library.syncedRideIDs()

        do {
            let catalog = try await transport.listRides()
            // Canceled = a newer sync superseded this one and owns the shared
            // state now — touch nothing (same rule at every check below).
            guard !Task.isCancelled else { return }
            onRideListRead()
            // The v2 header's truncation signal: some rides are unsyncable until
            // the rider frees space on the device (spec §7.4). Surface it from the
            // list read whether or not there's anything fresh to fetch.
            hiddenRideCount = catalog.hiddenRideCount

            let onDevice = catalog.rides
            let fresh = onDevice.filter { !syncedRideIDs.contains($0.id) }
            guard !fresh.isEmpty else {
                // H9 — a quiet toast, straight back to idle (no empty "done").
                syncState = .idle
                upToDateToastVisible = true
                return
            }

            syncProgress = SyncProgress(done: 0, total: fresh.count)
            let download = transport.downloadRides(fresh.map(\.id))
            activeDownload = download
            // A drop stalls the download streams open (that's what makes the
            // batch restartable, whole rides at a time). Watch the link and
            // surface H10 with what landed; the loop below just keeps awaiting
            // the stalled stream until Resume — or a new sync — moves things.
            // Held locally too: a superseded task must cancel ITS watch, never
            // the one a newer sync installed in the shared property.
            let dropWatch = Task { [weak self, transport] in
                for await state in transport.state
                where state == .outOfRange || state == .disconnected {
                    guard let self else { return }
                    if Task.isCancelled { break }
                    interruptSync()
                }
            }
            syncDropWatch = dropWatch
            var landed = 0
            do {
                for try await downloaded in download.rides {
                    // A superseding sync (or a fresh sync over a waiting
                    // interruption) cancels this task; a late ride yielded by the
                    // old stalled stream must not mutate the new sync's shared
                    // state (double-persist, clobbered progress / activeDownload).
                    guard !Task.isCancelled else { return }
                    syncedRideIDs.insert(downloaded.id)
                    library.markRideSynced(downloaded.id)
                    // Persist the canonical ride the moment it lands, so an
                    // interrupted batch keeps its partial across a relaunch
                    // (H10). The payload decodes through the device ride codec;
                    // bytes that don't parse keep the ride summary-only rather
                    // than dropping it (wire bytes are never the stored format).
                    if let summary = fresh.first(where: { $0.id == downloaded.id }) {
                        let decoded = try? RideObjectCodec.decode(
                            downloaded.payload, id: downloaded.id)
                        // The RideList summary stays canonical for display; the
                        // payload contributes the tracklog (and a preview, if
                        // the list entry came without one), plus the per-ride
                        // BLE-sensor summary (epic #707) the rideList entry
                        // doesn't carry — it only exists in the ride object's
                        // v2 header.
                        var ride = Ride(summary: summary, points: decoded?.points ?? [])
                        if ride.summary.trackPreview == nil {
                            ride.summary.trackPreview = decoded?.summary.trackPreview
                        }
                        if let decoded {
                            ride.summary.avgHeartRate = decoded.summary.avgHeartRate
                            ride.summary.maxHeartRate = decoded.summary.maxHeartRate
                            ride.summary.avgCadence = decoded.summary.avgCadence
                            ride.summary.avgPower = decoded.summary.avgPower
                            ride.summary.maxPower = decoded.summary.maxPower
                        }
                        library.saveRide(ride)
                        onRideLanded(ride)
                    }
                    landed += 1
                    syncProgress = SyncProgress(done: landed, total: fresh.count)
                }
            } catch {
                // A hard transfer failure — fall through; `landed` keeps the
                // partial batch either way.
            }
            // The transfer is over one way or another: the watch has done its
            // job (leaving it running would fire H10 on a later, harmless drop).
            dropWatch.cancel()
            guard !Task.isCancelled else { return }
            syncProgress = nil
            syncInterruption = nil
            activeDownload = nil
            let outcome = await download.handle.outcome
            // Re-check after the outcome await too: a superseding `sync()` can
            // land while this task is suspended here (its `handle.cancel()` is
            // exactly what resolves a superseded batch's outcome) — resuming
            // without this guard would clobber the new sync's `.syncing`/counts.
            guard !Task.isCancelled else { return }
            guard outcome == .completed else {
                syncState = .idle
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
            // Only the ride list read can land here (transfer-stream errors are
            // handled above) — no watch or download exists yet.
            guard !Task.isCancelled else { return }
            syncProgress = nil
            syncState = .idle
        }
    }

    /// The drop watch's H10 hand-off: freeze the counts into the banner state
    /// and bring the progress caption down. The download stays resumable.
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
