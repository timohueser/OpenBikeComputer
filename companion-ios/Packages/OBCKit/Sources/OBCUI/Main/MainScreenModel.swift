import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The main-screen state (B3, design C1/C2): the route/ride lists, the live
/// device cluster (name · battery · connection), search, and the sync button's
/// state machine. Depends only on `DeviceTransport` (the golden rule).
///
/// **Sync scope note:** B7 owns the real sync flow (H10 interrupted-banner +
/// resume). What lives here is the SYNC button contract — idle → syncing
/// ("N of M rides") → done ("Synced N new rides just now", ~2 s check) → idle —
/// driven off the `downloadRides` `TransferHandle`. "New" means not in the
/// `LibraryStore`'s synced set (B1S) — persistent, so a relaunch never
/// re-counts. A drop mid-sync keeps what landed (each ride persists as it
/// arrives) and returns the button to idle.
///
/// **Library rule (B1S):** the lists read the store first, then reconcile with
/// the device — the store is why S4 degrades to a banner over browsable
/// content instead of emptying, and why an H4 import survives a relaunch.
@MainActor @Observable
public final class MainScreenModel {
    /// The Planned | Tracked segmented split.
    public enum Tab: Int, Sendable {
        case planned = 0
        case tracked = 1
    }

    /// First-read lifecycle for the lists — S2 skeletons / S3 read error.
    public enum LoadState: Equatable, Sendable {
        case loading
        case loaded
        case failed
    }

    /// Ride-count progress for the syncing caption ("3 of 5 rides").
    public struct SyncProgress: Equatable, Sendable {
        public var done: Int
        public var total: Int
    }

    /// Pacing — injectable so the model tests run in milliseconds.
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

    /// Device name for the top bar + banner copy ("Trailhead").
    public private(set) var deviceName = "Your OBC"
    public private(set) var connection: ConnectionState = .connecting
    /// Battery percent, `nil` until the stream's first value.
    public private(set) var battery: Int?
    public private(set) var loadState: LoadState = .loading
    public private(set) var routes: [RouteSummary] = []
    public private(set) var rides: [RideSummary] = []
    public var tab: Tab = .planned
    public var searchText = ""
    public private(set) var syncState: OBCSyncButtonState = .idle
    /// Non-nil while syncing — feeds the amber "N of M rides" caption.
    public private(set) var syncProgress: SyncProgress?
    /// Non-nil after a successful sync — feeds "Synced N new rides just now".
    public private(set) var lastSyncCount: Int?
    /// H9: a sync found nothing new (bound to the transient toast).
    public var upToDateToastVisible = false

    // MARK: Derived

    /// The S4 rule: out of range / disconnected degrades to a banner over
    /// browsable content — never an error, never a blocker.
    public var showsDisconnectedBanner: Bool {
        connection == .outOfRange || connection == .disconnected
    }

    public var filteredRoutes: [RouteSummary] {
        filtered(routes, by: \.name)
    }

    public var filteredRides: [RideSummary] {
        filtered(rides, by: \.name)
    }

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let library: any LibraryStore
    private let timing: Timing
    /// Mirror of `library.syncedRideIDs()` — what makes the next sync's "new".
    @ObservationIgnored private var syncedRideIDs: Set<RideID> = []
    /// Mirror of the store's planned routes, keyed for the detail/rename paths.
    @ObservationIgnored private var plannedRecords: [RouteID: PlannedRouteRecord] = [:]
    /// Mirror of the store's synced rides (tracklogs stay empty until the S0
    /// ride codec lands — B7 fills them at decode time).
    @ObservationIgnored private var rideRecords: [RideID: Ride] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    @ObservationIgnored private var syncTask: Task<Void, Never>?
    @ObservationIgnored private var syncDropWatch: Task<Void, Never>?

    /// The default `library` keeps persistence out of previews and tests that
    /// don't care; the composition root always passes its chosen store.
    public init(
        transport: any DeviceTransport,
        library: any LibraryStore = InMemoryLibraryStore(),
        timing: Timing = Timing()
    ) {
        self.transport = transport
        self.library = library
        self.timing = timing
    }

    // MARK: Lifecycle

    /// Subscribe the live streams and load the library (call once, from the
    /// host's `.task`).
    public func start() {
        guard !started else { return }
        started = true

        // Library first (B1S): the lists are browsable before the device read
        // lands — or ever succeeds (offline relaunch, H4 pre-pairing import).
        let planned = library.plannedRoutes()
        plannedRecords = Dictionary(uniqueKeysWithValues: planned.map { ($0.id, $0) })
        let storedRides = library.rides()
        rideRecords = Dictionary(uniqueKeysWithValues: storedRides.map { ($0.id, $0) })
        syncedRideIDs = library.syncedRideIDs()
        routes = planned.map(\.summary)
        rides = storedRides.map(\.summary)

        streamTasks.append(Task { [transport] in
            for await state in transport.state { connection = state }
        })
        streamTasks.append(Task { [transport] in
            for await percent in transport.battery { battery = percent }
        })
        reload()
        // Identity after the first library read: a fault armed for "the first
        // read" (the S3 scenario) must hit the lists, not this fetch.
        let firstLoad = loadTask
        streamTasks.append(Task { [transport] in
            await firstLoad?.value
            if let info = try? await transport.deviceInfo() { deviceName = info.name }
        })
    }

    /// (Re)read both lists — also the S3 "Retry" action. Cached content stays
    /// up while the fresh read runs; only an *empty* library shows skeletons.
    public func reload() {
        loadTask?.cancel()
        loadState = .loading
        loadTask = Task { [transport] in
            do {
                async let routesRead = transport.listRoutes()
                async let ridesRead = transport.listRides()
                let (routes, rides) = try await (routesRead, ridesRead)
                guard !Task.isCancelled else { return }
                // Library-saved routes the device doesn't have yet (H4: saved
                // before/without a device) stay listed above the device's; a
                // saved route the device *does* list has been uploaded.
                let onDevice = Set(routes.map(\.id))
                let phoneOnly = plannedRecords.values
                    .filter { !onDevice.contains($0.id) }
                    .sorted { $0.addedAt > $1.addedAt }
                self.routes = phoneOnly.map(\.summary) + routes
                for id in plannedRecords.keys where onDevice.contains(id) {
                    markRouteUploaded(id)
                }
                self.rides = merged(deviceRides: rides)
                loadState = .loaded
            } catch {
                guard !Task.isCancelled else { return }
                loadState = .failed
            }
        }
    }

    // MARK: Sync (the SYNC button)

    /// Pull new tracked rides off the device. No-ops unless the link is up and
    /// no sync is running (the button is disabled when unreachable — S4 dims
    /// link-bound actions).
    public func sync() {
        guard connection == .connected, syncState != .syncing else { return }
        syncTask?.cancel()
        syncDropWatch?.cancel()
        let task = Task { await runSync() }
        syncTask = task
        // A drop stalls the download streams open (that's what makes them
        // resumable — B7's H10 flow). Watch the link and bail instead of
        // hanging; whatever already landed stays counted.
        syncDropWatch = Task { [transport] in
            for await state in transport.state
            where state == .outOfRange || state == .disconnected {
                task.cancel()
                break
            }
        }
    }

    private func runSync() async {
        syncState = .syncing
        lastSyncCount = nil

        do {
            let onDevice = try await transport.listRides()
            guard !Task.isCancelled else { syncState = .idle; return }
            rides = merged(deviceRides: onDevice)
            loadState = .loaded

            let fresh = onDevice.filter { !syncedRideIDs.contains($0.id) }
            guard !fresh.isEmpty else {
                // H9 — a quiet toast, straight back to idle (no empty "done").
                syncDropWatch?.cancel()
                syncState = .idle
                upToDateToastVisible = true
                return
            }

            syncProgress = SyncProgress(done: 0, total: fresh.count)
            let download = transport.downloadRides(fresh.map(\.id))
            var landed = 0
            do {
                for try await downloaded in download.rides {
                    syncedRideIDs.insert(downloaded.id)
                    library.markRideSynced(downloaded.id)
                    // Persist the canonical ride the moment it lands, so an
                    // interrupted batch keeps its partial across a relaunch
                    // (H10). Points stay empty until the S0 ride codec (B7)
                    // decodes `downloaded.payload` — wire bytes are never the
                    // stored format.
                    if let summary = fresh.first(where: { $0.id == downloaded.id }) {
                        let ride = Ride(summary: summary, points: [])
                        rideRecords[ride.id] = ride
                        library.saveRide(ride)
                    }
                    landed += 1
                    syncProgress = SyncProgress(done: landed, total: fresh.count)
                }
            } catch {
                // Cancellation (drop) or a hard transfer failure — fall
                // through; `landed` keeps the partial batch either way.
            }
            // The transfer is over one way or another: the watch has done its
            // job (leaving it running would cancel the done-hold below on a
            // later, harmless drop) and the progress caption comes down.
            syncDropWatch?.cancel()
            syncProgress = nil
            if Task.isCancelled {
                syncState = .idle
                return
            }
            guard await download.handle.outcome == .completed else {
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
            syncDropWatch?.cancel()
            syncProgress = nil
            syncState = .idle
        }
    }

    // MARK: Delete (H11 → H1, post-confirm)

    /// Remove a planned route — optimistic locally (list + library), then on
    /// the device.
    public func deleteRoute(_ id: RouteID) {
        routes.removeAll { $0.id == id }
        plannedRecords[id] = nil
        library.deletePlannedRoute(id)
        Task { [transport] in
            try? await transport.deleteRoute(id)
        }
    }

    /// Remove a tracked ride from the list + library. Device-side it's
    /// local-only for now (the transport has no ride delete) — the id stays
    /// marked synced so the next sync doesn't re-count it as new.
    public func deleteRide(_ id: RideID) {
        rides.removeAll { $0.id == id }
        rideRecords[id] = nil
        library.deleteRide(id)
        syncedRideIDs.insert(id)
        library.markRideSynced(id)
    }

    // MARK: Rename (H12) + import landing (E1) — phone-side library edits

    /// Rename a planned route in the list. Phone-local by design (H12:
    /// "renames locally, propagates to device on next upload") — no transport
    /// op, but a library-saved route persists the new name.
    public func renameRoute(_ id: RouteID, to name: String) {
        guard let index = routes.firstIndex(where: { $0.id == id }) else { return }
        routes[index].name = name
        if var record = plannedRecords[id] {
            record.summary.name = name
            plannedRecords[id] = record
            library.savePlannedRoute(record)
        }
    }

    /// Rename a tracked ride in the list — same phone-local rule as routes.
    public func renameRide(_ id: RideID, to name: String) {
        guard let index = rides.firstIndex(where: { $0.id == id }) else { return }
        rides[index].name = name
        if var ride = rideRecords[id] {
            ride.summary.name = name
            rideRecords[id] = ride
            library.saveRide(ride)
        }
    }

    /// Land a just-imported route at the top of Planned (E1 "Save to Planned")
    /// — and in the library, so it survives a relaunch and uploads later (H4).
    /// The full record (canonical geometry + source file) stays app-side; the
    /// device never had this route, so `routeDetail` can't answer for it.
    public func addImportedRoute(_ record: PlannedRouteRecord) {
        plannedRecords[record.id] = record
        library.savePlannedRoute(record)
        routes.removeAll { $0.id == record.id }
        routes.insert(record.summary, at: 0)
        tab = .planned
    }

    /// The kept detail for a library-saved route, with the summary refreshed
    /// from the live list (renames must show).
    public func importedDetail(for id: RouteID) -> RouteDetail? {
        guard let record = plannedRecords[id] else { return nil }
        var detail = record.detail()
        if let live = routes.first(where: { $0.id == id }) { detail.summary = live }
        return detail
    }

    /// The device took a copy (a B5 upload completed, or a reconcile saw the
    /// route in the device's list) — remembered so B3 can dress "not on
    /// device yet" states and re-offer the upload.
    public func markRouteUploaded(_ id: RouteID) {
        guard var record = plannedRecords[id], !record.uploadedToDevice else { return }
        record.uploadedToDevice = true
        plannedRecords[id] = record
        library.savePlannedRoute(record)
    }

    // MARK: Helpers

    /// The Tracked list: the device's rides plus library-synced rides the
    /// device no longer holds — the phone is the archive, so a ride outlives
    /// its device-side copy.
    private func merged(deviceRides: [RideSummary]) -> [RideSummary] {
        let onDevice = Set(deviceRides.map(\.id))
        let archived = rideRecords.values.map(\.summary)
            .filter { !onDevice.contains($0.id) }
            .sorted { $0.date > $1.date }
        return deviceRides + archived
    }

    private func filtered<T>(_ items: [T], by name: KeyPath<T, String>) -> [T] {
        let query = searchText.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return items }
        return items.filter { $0[keyPath: name].localizedCaseInsensitiveContains(query) }
    }
}
