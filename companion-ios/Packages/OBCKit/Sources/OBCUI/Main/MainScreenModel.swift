import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The main-screen state (B3, design C1/C2): the route/ride lists, the live
/// device cluster (name · battery · connection), search, and the sync button's
/// state machine. Depends only on `DeviceTransport` (the golden rule).
///
/// **Sync scope note:** B7 owns the real sync flow (ride store, H10
/// interrupted-banner + resume). What lives here is the SYNC button contract —
/// idle → syncing ("N of M rides") → done ("Synced N new rides just now",
/// ~2 s check) → idle — driven off the `downloadRides` `TransferHandle`.
/// "New" means not downloaded by this app session yet; a second sync with
/// nothing new is the quiet H9 toast. A drop mid-sync keeps what landed
/// (partial data is never lost) and returns the button to idle.
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
    private let timing: Timing
    /// Rides this app session has pulled off the device — what makes the next
    /// sync's "new". B7 replaces this with the persistent ride store.
    @ObservationIgnored private var syncedRideIDs: Set<RideID> = []
    /// Full detail for routes saved from an import this session (see
    /// `addImportedRoute`). B7's library store takes this over too.
    @ObservationIgnored private var importedDetails: [RouteID: RouteDetail] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    @ObservationIgnored private var syncTask: Task<Void, Never>?
    @ObservationIgnored private var syncDropWatch: Task<Void, Never>?

    public init(transport: any DeviceTransport, timing: Timing = Timing()) {
        self.transport = transport
        self.timing = timing
    }

    // MARK: Lifecycle

    /// Subscribe the live streams and load the library (call once, from the
    /// host's `.task`).
    public func start() {
        guard !started else { return }
        started = true

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
                self.routes = routes
                self.rides = rides
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
            rides = onDevice
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
                for try await ride in download.rides {
                    syncedRideIDs.insert(ride.id)
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

    /// Remove a planned route — optimistic locally, then on the device.
    public func deleteRoute(_ id: RouteID) {
        routes.removeAll { $0.id == id }
        importedDetails[id] = nil
        Task { [transport] in
            try? await transport.deleteRoute(id)
        }
    }

    /// Remove a tracked ride from the list. Local-only for now: the transport
    /// has no ride delete, and the app-side ride store is B7's — the id stays
    /// marked synced so the next sync doesn't re-count it as new.
    public func deleteRide(_ id: RideID) {
        rides.removeAll { $0.id == id }
        syncedRideIDs.insert(id)
    }

    // MARK: Rename (H12) + import landing (E1) — session-local library edits

    /// Rename a planned route in the list. Local by design (H12: "renames
    /// locally, propagates to device on next upload") — no transport op.
    public func renameRoute(_ id: RouteID, to name: String) {
        guard let index = routes.firstIndex(where: { $0.id == id }) else { return }
        routes[index].name = name
    }

    /// Rename a tracked ride in the list — same local-only rule as routes.
    public func renameRide(_ id: RideID, to name: String) {
        guard let index = rides.firstIndex(where: { $0.id == id }) else { return }
        rides[index].name = name
    }

    /// Land a just-imported route at the top of Planned (E1 "Save to Planned").
    /// The full detail (waypoints + profile) stays app-side — the device never
    /// had this route, so `routeDetail` can't answer for it. Session-scoped
    /// like rename: the persistent phone-side library is B7's.
    public func addImportedRoute(_ detail: RouteDetail) {
        importedDetails[detail.summary.id] = detail
        routes.removeAll { $0.id == detail.summary.id }
        routes.insert(detail.summary, at: 0)
        tab = .planned
    }

    /// The kept detail for a route saved from an import this session, with the
    /// summary refreshed from the live list (renames must show).
    public func importedDetail(for id: RouteID) -> RouteDetail? {
        guard var detail = importedDetails[id] else { return nil }
        if let live = routes.first(where: { $0.id == id }) { detail.summary = live }
        return detail
    }

    // MARK: Helpers

    private func filtered<T>(_ items: [T], by name: KeyPath<T, String>) -> [T] {
        let query = searchText.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return items }
        return items.filter { $0[keyPath: name].localizedCaseInsensitiveContains(query) }
    }
}
