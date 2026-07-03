import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The main-screen state (B3, design C1/C2): the route/ride lists, the live
/// device cluster (name · battery · connection), search, and the sync button's
/// state machine. Depends only on `DeviceTransport` (the golden rule).
///
/// **Sync (B7):** the SYNC button contract — idle → syncing ("N of M rides") →
/// done ("Synced N new rides just now", ~2 s check) → idle — driven off the
/// `downloadRides` `RideDownload`. "New" means not in the `LibraryStore`'s
/// synced set (B1S) — persistent, so a relaunch never re-counts. Each landed
/// payload decodes through `RideObjectCodec` into the canonical `Ride`
/// and persists at once, so a drop mid-batch keeps what arrived (H10) by
/// construction. A drop surfaces as `syncInterruption` ("Got 2 of 5 rides." +
/// Resume); `resumeSync()` restarts the stalled batch at **whole-ride
/// granularity** — rides that fully landed stay, the rest are re-sent whole
/// (transfers restart, not resume — the spec's principle 4).
///
/// **The lists' two sources of truth (#289):**
/// - **Planned routes are library-first.** The list shows exactly the phone's
///   saved routes; `listRoutes()` (the device catalog, device-namespace ids) is
///   consulted *only* to reconcile each record's `deviceObjectID` — lighting
///   and clearing the C1 "on device" badge — never to add rows. A route that
///   exists only on the device (another phone's upload, a side-loaded file)
///   isn't the app's to manage and never appears.
/// - **Rides are device-first**: the device's list merged over the library's
///   archive (the phone keeps rides the device no longer holds), minus
///   phone-side tombstones.
///
/// That split is also why S4 degrades to a banner over browsable content
/// instead of emptying, and why an H4 import survives a relaunch.
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

    /// H10 — a sync the link dropped out from under. Feeds the warning banner
    /// ("Sync interrupted. Got 2 of 5 rides." + Resume). What landed is already
    /// persisted; `resumeSync()` continues the rest.
    public struct SyncInterruption: Equatable, Sendable {
        public var landed: Int
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
    /// Ids of planned routes the device holds a copy of — drives the "on device"
    /// badge (C1). Observable (unlike the `plannedRecords` mirror) so the badge
    /// lights the instant an upload commits.
    public private(set) var uploadedRouteIDs: Set<RouteID> = []
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
    /// H10: non-nil while a dropped sync waits for Resume — replaces the S4
    /// banner (one banner at a time; this one carries the link story too).
    public private(set) var syncInterruption: SyncInterruption?

    // MARK: Derived

    /// The S4 rule: out of range / disconnected degrades to a banner over
    /// browsable content — never an error, never a blocker. While a dropped
    /// sync waits for Resume, the H10 banner tells the link story instead.
    public var showsDisconnectedBanner: Bool {
        (connection == .outOfRange || connection == .disconnected) && syncInterruption == nil
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
    /// Mirror of `library.deletedRideIDs()` — device rides deleted on the
    /// phone; the merge hides them so a sync/reload can't resurrect them.
    @ObservationIgnored private var deletedRideIDs: Set<RideID> = []
    /// Mirror of the store's planned routes, keyed for the detail/rename paths.
    @ObservationIgnored private var plannedRecords: [RouteID: PlannedRouteRecord] = [:]
    /// Mirror of the store's synced rides — tracklogs filled at decode time
    /// (`RideObjectCodec`, B7).
    @ObservationIgnored private var rideRecords: [RideID: Ride] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    @ObservationIgnored private var syncTask: Task<Void, Never>?
    @ObservationIgnored private var syncDropWatch: Task<Void, Never>?
    /// The running (or dropped-but-resumable) download — `resumeSync()` signals
    /// its handle; the consuming loop in `runSync` is still awaiting its stream.
    @ObservationIgnored private var activeDownload: RideDownload?

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
        uploadedRouteIDs = Set(planned.filter(\.uploadedToDevice).map(\.id))
        let storedRides = library.rides()
        rideRecords = Dictionary(uniqueKeysWithValues: storedRides.map { ($0.id, $0) })
        syncedRideIDs = library.syncedRideIDs()
        deletedRideIDs = library.deletedRideIDs()
        routes = plannedList()
        rides = storedRides.map(\.summary)

        streamTasks.append(Task { [transport] in
            var previous: ConnectionState?
            for await state in transport.state {
                connection = state
                // A regained link (never the stream's replayed first value):
                // re-read the lists — the reconnect is what makes the badges
                // and ride list trustworthy again.
                if state == .connected, let was = previous, was != .connected { reload() }
                previous = state
            }
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
                let (deviceRoutes, deviceRides) = try await (routesRead, ridesRead)
                guard !Task.isCancelled else { return }
                // Planned stays library-only (#289) — the device catalog only
                // reconciles each record's on-device link (badge on AND off).
                reconcileOnDevice(with: deviceRoutes)
                routes = plannedList()
                rides = merged(deviceRides: deviceRides)
                loadState = .loaded
            } catch {
                guard !Task.isCancelled else { return }
                loadState = .failed
            }
        }
    }

    /// True-up every record's `deviceObjectID` against the device's live catalog
    /// (device-namespace ids): a copy deleted out from under us (another phone,
    /// the EchoHarness) clears the badge; a record whose id is still listed keeps
    /// it. Ids are durable across device reboots (spec §4.1), so absence really
    /// means "gone", not "renumbered".
    private func reconcileOnDevice(with deviceRoutes: [RouteSummary]) {
        let onDevice = Set(deviceRoutes.compactMap { UInt16($0.id.rawValue) })
        for (id, var record) in plannedRecords {
            guard let objectID = record.deviceObjectID, !onDevice.contains(objectID) else { continue }
            record.deviceObjectID = nil
            plannedRecords[id] = record
            library.savePlannedRoute(record)
        }
        uploadedRouteIDs = Set(plannedRecords.values.filter(\.uploadedToDevice).map(\.id))
    }

    /// The Planned rows: the library's records, newest first.
    private func plannedList() -> [RouteSummary] {
        plannedRecords.values.sorted { $0.addedAt > $1.addedAt }.map(\.summary)
    }

    // MARK: Sync (the SYNC button)

    /// Pull new tracked rides off the device. No-ops unless the link is up and
    /// no sync is running (the button is disabled when unreachable — S4 dims
    /// link-bound actions). Starting fresh over a waiting interruption is fine:
    /// what landed is marked synced, so the new batch is exactly the remainder.
    public func sync() {
        guard connection == .connected, syncState != .syncing else { return }
        syncTask?.cancel()
        syncDropWatch?.cancel()
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

        do {
            let onDevice = try await transport.listRides()
            // Canceled = a newer sync superseded this one and owns the shared
            // state now — touch nothing (same rule at every check below).
            guard !Task.isCancelled else { return }
            rides = merged(deviceRides: onDevice)
            loadState = .loaded

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
            let dropWatch = Task { [transport] in
                for await state in transport.state
                where state == .outOfRange || state == .disconnected {
                    if Task.isCancelled { break }
                    interruptSync()
                }
            }
            syncDropWatch = dropWatch
            var landed = 0
            do {
                for try await downloaded in download.rides {
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
                        // the list entry came without one).
                        var ride = Ride(summary: summary, points: decoded?.points ?? [])
                        if ride.summary.trackPreview == nil {
                            ride.summary.trackPreview = decoded?.summary.trackPreview
                        }
                        rideRecords[ride.id] = ride
                        library.saveRide(ride)
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

    // MARK: Delete (H11 → H1, post-confirm)

    /// Remove a planned route from the phone (list + library). **Never** from
    /// the device — H1's promise is "If it's already on the device, it stays
    /// there", mirroring the ride rule in reverse (each side keeps its own
    /// copies; the record and its badge die with the library entry).
    public func deleteRoute(_ id: RouteID) {
        routes.removeAll { $0.id == id }
        plannedRecords[id] = nil
        uploadedRouteIDs.remove(id)
        library.deletePlannedRoute(id)
    }

    /// Remove a tracked ride from the phone (list + library) — **never** from
    /// the device: the SD-card copy stays. The tombstone keeps it that way
    /// durably — the id stays marked synced (the next sync doesn't re-download
    /// it) and marked deleted (the merge doesn't re-list the device's copy).
    public func deleteRide(_ id: RideID) {
        rides.removeAll { $0.id == id }
        rideRecords[id] = nil
        library.deleteRide(id)
        syncedRideIDs.insert(id)
        library.markRideSynced(id)
        deletedRideIDs.insert(id)
        library.markRideDeleted(id)
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
        // A re-import that replaces an existing route keeps its `deviceObjectID`
        // (and thus its badge); a fresh import isn't on the device yet.
        if record.uploadedToDevice { uploadedRouteIDs.insert(record.id) } else { uploadedRouteIDs.remove(record.id) }
        routes.removeAll { $0.id == record.id }
        routes.insert(record.summary, at: 0)
        tab = .planned
    }

    /// A saved planned route whose name matches `name` (case-insensitively) — the
    /// import edge asks so it can offer "replace" instead of a duplicate.
    public func plannedRoute(named name: String) -> PlannedRouteRecord? {
        let target = name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return plannedRecords.values.first { $0.summary.name.lowercased() == target }
    }

    /// Whether the device holds a copy of this planned route (drives the C1 badge).
    public func isUploaded(_ id: RouteID) -> Bool { uploadedRouteIDs.contains(id) }

    /// The kept detail for a library-saved route, with the summary refreshed
    /// from the live list (renames must show).
    public func importedDetail(for id: RouteID) -> RouteDetail? {
        guard let record = plannedRecords[id] else { return nil }
        var detail = record.detail()
        if let live = routes.first(where: { $0.id == id }) { detail.summary = live }
        return detail
    }

    /// The canonical parsed geometry a library-saved route re-encodes to OBCR for
    /// upload (B12). `nil` for a device-listed route the phone never imported —
    /// that copy already lives on the device.
    public func plannedGeometry(for id: RouteID) -> ImportedRoute? {
        plannedRecords[id]?.route
    }

    /// The device object id this planned route is stored under, if any — threaded
    /// into a re-upload so it replaces that object instead of duplicating.
    public func plannedDeviceObjectID(for id: RouteID) -> UInt16? {
        plannedRecords[id]?.deviceObjectID
    }

    /// H3 write-through from Settings (B8) — the top bar shows the new device
    /// name at once; Settings owns the config write and the bond record.
    public func deviceRenamed(to name: String) {
        deviceName = name
    }

    /// A B5 upload committed — record the device object id it landed under (the
    /// durable "on device" link) so the C1 badge lights and a later re-upload
    /// replaces that object. Idempotent; a new id (re-upload after a device-side
    /// change) overwrites the old.
    public func markRouteUploaded(_ id: RouteID, objectID: UInt16) {
        guard var record = plannedRecords[id] else { return }
        record.deviceObjectID = objectID
        plannedRecords[id] = record
        uploadedRouteIDs.insert(id)
        library.savePlannedRoute(record)
    }

    // MARK: Helpers

    /// The Tracked list: the device's rides plus library-synced rides the
    /// device no longer holds — the phone is the archive, so a ride outlives
    /// its device-side copy. Rides deleted on the phone are tombstoned out:
    /// the device still lists them (its copy stays), but they must not
    /// resurrect here.
    private func merged(deviceRides: [RideSummary]) -> [RideSummary] {
        let kept = deviceRides.filter { !deletedRideIDs.contains($0.id) }
        let onDevice = Set(kept.map(\.id))
        let archived = rideRecords.values.map(\.summary)
            .filter { !onDevice.contains($0.id) }
            .sorted { $0.date > $1.date }
        return kept + archived
    }

    private func filtered<T>(_ items: [T], by name: KeyPath<T, String>) -> [T] {
        let query = searchText.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return items }
        return items.filter { $0[keyPath: name].localizedCaseInsensitiveContains(query) }
    }
}
