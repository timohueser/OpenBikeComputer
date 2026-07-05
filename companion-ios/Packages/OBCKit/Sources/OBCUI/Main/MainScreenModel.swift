import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The main-screen state (B3, design C1/C2): the route/ride lists, the live
/// device cluster (name · battery · connection), search, and the phone-side
/// library edits (rename/delete/import landing). Depends only on
/// `DeviceTransport` (the golden rule).
///
/// **Sync (B7)** lives in `RideSyncCoordinator` (#358), exposed whole as
/// `sync` — the view reads `sync.syncState` etc. directly rather than through
/// duplicated properties. The coordinator persists each landed ride itself;
/// this model only mirrors landed rides into its in-memory Tracked list (the
/// `onRideLanded` seam) and vetoes sync on a protocol mismatch (`canSync`).
///
/// **Both lists are library-first (#289, extended to rides in #296):**
/// - **Planned routes** show exactly the phone's saved routes; `listRoutes()`
///   (the device catalog, keyed by device object ids) is consulted *only* to reconcile
///   each record's `deviceObjectID` — lighting and clearing the C1 "on device"
///   badge — never to add rows. A route that exists only on the device (another
///   phone's upload, a side-loaded file) isn't the app's to manage and never
///   appears.
/// - **Tracked rides** show exactly the rides the phone has **synced** (its
///   library), newest first, minus phone-side tombstones. A ride sitting on the
///   device but not yet downloaded is *not* a row — it has no tracklog or
///   preview yet, only summary stats, and a half-empty card is worse than none.
///   `listRides()` drives the *sync* (what to fetch on Sync), never the rows;
///   nothing downloads until the user presses Sync.
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

    /// #303 — a connected device whose reported `protocol_version` doesn't match
    /// `OBCProtocol.version`. Feeds the incompatibility banner and disables sync;
    /// the app must never decode an incompatible object (`OBCProtocol.md` →
    /// *Versioning*). `found > expected` means the device is ahead (update the
    /// app); `found < expected` means it's behind (update the OBC).
    public struct ProtocolMismatch: Equatable, Sendable {
        public var expected: UInt16
        public var found: UInt16
    }

    // MARK: Observable state

    /// Device name for the top bar + banner copy ("Trailhead").
    public private(set) var deviceName = "Your OBC"
    public private(set) var connection: ConnectionState = .connecting
    /// Battery percent, `nil` until the stream's first value.
    public private(set) var battery: Int?
    public private(set) var loadState: LoadState = .loading
    public private(set) var routes: [RouteSummary] = []
    /// Each planned route's proven device-copy state — drives the C1 badge
    /// (check = up to date, refresh = on device but out of date). Observable
    /// (unlike the `plannedRecords` mirror) so the badge moves the instant an
    /// upload commits or a rename/re-import changes the content.
    public private(set) var onDevice: [RouteID: OnDeviceState] = [:]
    public private(set) var rides: [RideSummary] = []
    /// Recently Deleted (#292): trashed rides, most recently trashed first —
    /// the trash screen's rows and the Tracked tab's entry-row count.
    public private(set) var trashedRides: [RideSummary] = []
    public var tab: Tab = .planned
    public var searchText = ""
    /// #303: non-nil once a connected device reports an incompatible
    /// `protocol_version` — drives the incompatibility banner and disables sync.
    public private(set) var protocolMismatch: ProtocolMismatch?

    /// The ride-sync state machine (B7/#358) — exposed whole so the view reads
    /// `sync.syncState`, `sync.syncProgress`, … without duplicated mirrors.
    public let sync: RideSyncCoordinator

    // MARK: Derived

    /// The S4 rule: out of range / disconnected degrades to a banner over
    /// browsable content — never an error, never a blocker. While a dropped
    /// sync waits for Resume, the H10 banner tells the link story instead.
    public var showsDisconnectedBanner: Bool {
        (connection == .outOfRange || connection == .disconnected) && sync.syncInterruption == nil
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
    /// Desired-name reconcile (#361), run once per established connection —
    /// the logic lives in `DeviceNameReconciler`; this model only owns the
    /// "connection established" trigger. `nil` (tests/previews) skips it.
    private let nameReconciler: DeviceNameReconciler?
    /// Mirror of `library.deletedRideIDs()` — device rides deleted on the
    /// phone; the merge hides them so a sync/reload can't resurrect them.
    @ObservationIgnored private var deletedRideIDs: Set<RideID> = []
    /// Mirror of `library.trashedRideIDs()` — rides in Recently Deleted (#292),
    /// with when each was trashed (orders the trash, drives the retention purge).
    @ObservationIgnored private var trashedRideIDs: [RideID: Date] = [:]
    /// Mirror of the store's planned routes, keyed for the detail/rename paths.
    @ObservationIgnored private var plannedRecords: [RouteID: PlannedRouteRecord] = [:]
    /// Mirror of the store's synced-ride **summaries** — tracklogs stay on disk
    /// and load one-ride-at-a-time through `rideGeometry(for:)` (#360); pinning
    /// a season of decoded points here is exactly what that issue removed.
    @ObservationIgnored private var rideSummaries: [RideID: RideSummary] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var loadTask: Task<Void, Never>?

    /// How long a trashed ride survives before the start-up sweep removes it
    /// for good — the trash screen's copy quotes this.
    public static let trashRetentionDays = 30

    /// The trash retention clock — injectable so tests can age the trash
    /// without waiting a month.
    private let now: () -> Date

    /// The default `library` keeps persistence out of previews and tests that
    /// don't care; the composition root always passes its chosen store.
    public init(
        transport: any DeviceTransport,
        library: any LibraryStore = InMemoryLibraryStore(),
        syncTiming: RideSyncCoordinator.Timing = RideSyncCoordinator.Timing(),
        nameReconciler: DeviceNameReconciler? = nil,
        transferActivity: TransferActivity? = nil,
        now: @escaping () -> Date = Date.init
    ) {
        self.transport = transport
        self.library = library
        self.nameReconciler = nameReconciler
        self.now = now
        self.sync = RideSyncCoordinator(
            transport: transport, library: library, timing: syncTiming,
            activity: transferActivity
        )
        // The coordinator's seams back into this model — weak, so the closures
        // the model's own coordinator holds can never pin the model.
        sync.canSync = { [weak self] in self?.protocolMismatch == nil }
        sync.onRideListRead = { [weak self] in self?.loadState = .loaded }
        // A landed ride is already persisted (the coordinator's job) — mirror
        // it into the session's list so newly synced rides surface at once,
        // not only after the next reload.
        sync.onRideLanded = { [weak self] ride in
            guard let self else { return }
            // Only the summary is kept — the ride (points included) is already
            // persisted by the coordinator; the detail reads points back through
            // the store when it opens.
            rideSummaries[ride.id] = ride.summary
            rides = trackedList()
        }
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
        refreshOnDeviceStates()
        let storedSummaries = library.rideSummaries()
        rideSummaries = Dictionary(uniqueKeysWithValues: storedSummaries.map { ($0.id, $0) })
        deletedRideIDs = library.deletedRideIDs()
        trashedRideIDs = library.trashedRideIDs()
        purgeExpiredTrash()
        routes = plannedList()
        rides = trackedList()
        trashedRides = trashedList()

        // Open-ended stream loops are `[weak self]` + per-iteration `guard let
        // self` (the SettingsModel/RouteDetailModel convention) — the streams
        // never finish, so a strong capture would pin the model for the session.
        streamTasks.append(Task { [weak self, transport] in
            var previous: ConnectionState?
            for await state in transport.state {
                guard let self else { return }
                connection = state
                // A regained link (never the stream's replayed first value):
                // re-read the lists — the reconnect is what makes the badges
                // and ride list trustworthy again — and run the desired-name
                // reconcile (#361), the once-per-connect self-heal for a
                // rename whose config write never landed. Fire-and-forget:
                // the reconciler captures only its own transport + bond
                // store, never this model.
                if state == .connected, let was = previous, was != .connected {
                    reload()
                    if let nameReconciler {
                        Task { await nameReconciler.reconcile() }
                    }
                }
                previous = state
            }
        })
        streamTasks.append(Task { [weak self, transport] in
            for await percent in transport.battery {
                guard let self else { return }
                battery = percent
            }
        })
        reload()
        // Identity after the first library read: a fault armed for "the first
        // read" (the S3 scenario) must hit the lists, not this fetch. The same
        // read carries the protocol-version check (#303) — surfaced here, on
        // connect, where `deviceInfo()` is consumed.
        let firstLoad = loadTask
        streamTasks.append(Task { [weak self, transport] in
            await firstLoad?.value
            guard let info = try? await transport.deviceInfo() else { return }
            guard let self else { return }
            deviceName = info.name
            if case let .protocolMismatch(expected, found)? =
                OBCProtocol.versionMismatch(reportedBy: info.protocolVersion) {
                protocolMismatch = ProtocolMismatch(expected: expected, found: found)
            }
        })
        // Desired-name reconcile for the *launch* connection (#361) — the
        // reconnect edge above never fires for the stream's replayed first
        // value. After the first load for the same reason as the identity
        // read: a fault armed for "the first read" (S3) must hit the lists,
        // not this config read. Launched disconnected, the pass skips
        // silently and the reconnect edge owns every later connect.
        if let nameReconciler {
            streamTasks.append(Task {
                await firstLoad?.value
                await nameReconciler.reconcile()
            })
        }
    }

    deinit {
        // The sync coordinator cancels its own tasks in its deinit — this
        // model owns (and cancels) only what it runs itself.
        streamTasks.forEach { $0.cancel() }
        loadTask?.cancel()
    }

    /// (Re)read both lists — also the S3 "Retry" action. Cached content stays
    /// up while the fresh read runs; only an *empty* library shows skeletons.
    public func reload() {
        loadTask?.cancel()
        // An incompatible device (#303): don't decode its objects — keep the
        // library-first content up and let the banner explain. The first load
        // (before the version is read) may still run; every reload after the
        // mismatch is known is gated here.
        guard protocolMismatch == nil else {
            loadState = .loaded
            return
        }
        loadState = .loading
        loadTask = Task { [transport] in
            do {
                // Only the route catalog is read here: Planned reconciles its
                // on-device badges against it, and Tracked is library-first
                // (#296) so its rows come from the local library — the device's
                // rides are pulled only by Sync, never on a plain (re)load.
                let deviceRoutes = try await transport.listRoutes()
                guard !Task.isCancelled else { return }
                reconcileOnDevice(with: deviceRoutes)
                routes = plannedList()
                rides = trackedList()
                loadState = .loaded
            } catch {
                guard !Task.isCancelled else { return }
                loadState = .failed
            }
        }
    }

    /// True-up every record's `deviceObjectID` against the device's live catalog
    /// (device object ids): a copy deleted out from under us (another phone,
    /// the EchoHarness) clears the badge; a record whose id is still listed keeps
    /// it. Ids are durable across device reboots (spec §4.1), so absence really
    /// means "gone", not "renumbered".
    private func reconcileOnDevice(with deviceRoutes: [RouteCatalogEntry]) {
        let listed = Set(deviceRoutes.map(\.id))
        for (id, var record) in plannedRecords {
            guard let objectID = record.deviceObjectID, !listed.contains(objectID) else { continue }
            record.deviceObjectID = nil
            record.uploadedCRC32 = nil
            plannedRecords[id] = record
            library.savePlannedRoute(record)
        }
        refreshOnDeviceStates()
    }

    /// Recompute every record's proven device-copy state — called whenever a
    /// record's content or its device link moves (load, reconcile, upload,
    /// rename, re-import, delete). The payload encode behind the CRC only runs
    /// for records the device actually holds with a known fingerprint.
    private func refreshOnDeviceStates() {
        onDevice = plannedRecords.mapValues { record in
            OnDeviceState.determine(
                deviceObjectID: record.deviceObjectID,
                uploadedCRC32: record.uploadedCRC32,
                currentCRC: { RouteObjectCodec.payloadCRC(for: record) }
            )
        }
    }

    /// The Planned rows: the library's records, newest first.
    private func plannedList() -> [RouteSummary] {
        plannedRecords.values.sorted { $0.addedAt > $1.addedAt }.map(\.summary)
    }

    // MARK: Delete (H11 → H1, post-confirm)

    /// Remove a planned route from the phone (list + library). **Never** from
    /// the device — H1's promise is "If it's already on the device, it stays
    /// there", mirroring the ride rule in reverse (each side keeps its own
    /// copies; the record and its badge die with the library entry).
    public func deleteRoute(_ id: RouteID) {
        routes.removeAll { $0.id == id }
        plannedRecords[id] = nil
        onDevice[id] = nil
        library.deletePlannedRoute(id)
    }

    /// Move a tracked ride to Recently Deleted (#292) — recoverable, and
    /// **never** touching the device: the SD-card copy stays. The stored files
    /// stay too (that's what makes Recover instant); only the Tracked list
    /// hides the ride. The id stays marked synced so the next sync doesn't
    /// re-download it while — or after — it sits in the trash; the coordinator
    /// re-reads `syncedRideIDs()` at the start of every sync, so
    /// `markRideSynced` here is the whole hand-off.
    public func deleteRide(_ id: RideID) {
        rides.removeAll { $0.id == id }
        let date = now()
        trashedRideIDs[id] = date
        library.markRideTrashed(id, at: date)
        library.markRideSynced(id)
        trashedRides = trashedList()
    }

    /// Put a trashed ride back in Tracked — just clearing the trash mark; the
    /// stored summary and tracklog never moved.
    public func recoverRide(_ id: RideID) {
        trashedRideIDs[id] = nil
        library.unmarkRideTrashed(id)
        rides = trackedList()
        trashedRides = trashedList()
    }

    /// Permanently delete a trashed ride: the stored files go, and the durable
    /// tombstone takes over — the id stays marked synced (the next sync doesn't
    /// re-download it) and is marked deleted (the merge doesn't re-list the
    /// device's copy). What `deleteRide` did before the trash existed (#292).
    public func deleteRideForever(_ id: RideID) {
        trashedRideIDs[id] = nil
        library.unmarkRideTrashed(id)
        rideSummaries[id] = nil
        library.deleteRide(id)
        deletedRideIDs.insert(id)
        library.markRideDeleted(id)
        trashedRides = trashedList()
    }

    /// The start-up retention sweep: anything trashed more than
    /// `trashRetentionDays` ago is removed for good.
    private func purgeExpiredTrash() {
        let cutoff = now().addingTimeInterval(-TimeInterval(Self.trashRetentionDays) * 86_400)
        for (id, date) in trashedRideIDs where date < cutoff {
            deleteRideForever(id)
        }
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
            // The name rides in the upload payload: a rename out-dates the
            // device copy until the next push updates it.
            refreshOnDeviceStates()
        }
    }

    /// Rename a tracked ride in the list — same phone-local rule as routes.
    /// A summary-only write (#360): the tracklog on disk is untouched.
    public func renameRide(_ id: RideID, to name: String) {
        guard let index = rides.firstIndex(where: { $0.id == id }) else { return }
        rides[index].name = name
        if var summary = rideSummaries[id] {
            summary.name = name
            rideSummaries[id] = summary
            library.saveRideSummary(summary)
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
        refreshOnDeviceStates()
        routes.removeAll { $0.id == record.id }
        routes.insert(record.summary, at: 0)
        tab = .planned
    }

    /// A saved planned route whose name matches `name` (case-insensitively) — the
    /// import edge asks so it can offer "replace" instead of a duplicate.
    public func plannedRoute(named name: String) -> PlannedRouteRecord? {
        plannedRecords.values.plannedRoute(named: name)
    }

    /// Whether the device holds a copy of this planned route (drives the C1 badge).
    public func isUploaded(_ id: RouteID) -> Bool { onDeviceState(id) != .notOnDevice }

    /// The proven device-copy state behind the C1 badge.
    public func onDeviceState(_ id: RouteID) -> OnDeviceState { onDevice[id] ?? .notOnDevice }

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

    /// A synced ride's full tracklog (#294 follow-up) — the interactive map
    /// draws this, never the downsampled `trackPreview`. Loaded from the store
    /// on demand (#360): called once per detail push, so a synchronous one-file
    /// read is fine — only the list rows must never pay for tracklogs. `nil`
    /// when the ride hasn't landed (shouldn't happen for a row the detail
    /// screen can open) or carries no points (a pre-sync-codec ride); the
    /// detail degrades to the preview's coordinates either way, not a missing map.
    public func rideGeometry(for id: RideID) -> [Coordinate]? {
        let points = library.ridePoints(id)?.map(\.coordinate)
        return (points?.isEmpty ?? true) ? nil : points
    }

    /// The device object id this planned route is stored under, if any — threaded
    /// into a re-upload so it replaces that object instead of duplicating.
    public func plannedDeviceObjectID(for id: RouteID) -> DeviceObjectID? {
        plannedRecords[id]?.deviceObjectID
    }

    /// The committed upload fingerprint behind that link — threaded into the
    /// detail so its button can tell "up to date" from "out of date".
    public func plannedUploadedCRC32(for id: RouteID) -> UInt32? {
        plannedRecords[id]?.uploadedCRC32
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
    public func markRouteUploaded(_ id: RouteID, objectID: DeviceObjectID, crc32: UInt32) {
        guard var record = plannedRecords[id] else { return }
        record.deviceObjectID = objectID
        record.uploadedCRC32 = crc32
        plannedRecords[id] = record
        library.savePlannedRoute(record)
        refreshOnDeviceStates()
    }

    // MARK: Helpers

    /// The Tracked list: exactly the rides the phone has **synced** (its
    /// library), newest first — library-first, like Planned (#289/#296). A ride
    /// on the device but not yet downloaded is deliberately *not* here: it has
    /// only summary stats, no tracklog or preview, and a half-empty card is
    /// worse than none (the device's `listRides()` drives Sync, never the rows).
    /// Permanently deleted rides are already gone from `rideSummaries`; the
    /// tombstone filter is belt-and-suspenders. Trashed rides *are* still in
    /// `rideSummaries` (their files stay for Recover) — the trash filter is
    /// what hides them here.
    private func trackedList() -> [RideSummary] {
        rideSummaries.values
            .filter { !deletedRideIDs.contains($0.id) && trashedRideIDs[$0.id] == nil }
            .sorted { $0.date > $1.date }
    }

    /// The Recently Deleted rows: trashed rides, most recently trashed first.
    private func trashedList() -> [RideSummary] {
        trashedRideIDs
            .sorted { $0.value > $1.value }
            .compactMap { rideSummaries[$0.key] }
    }

    private func filtered<T>(_ items: [T], by name: KeyPath<T, String>) -> [T] {
        let query = searchText.trimmingCharacters(in: .whitespaces)
        guard !query.isEmpty else { return items }
        return items.filter { $0[keyPath: name].localizedCaseInsensitiveContains(query) }
    }
}
