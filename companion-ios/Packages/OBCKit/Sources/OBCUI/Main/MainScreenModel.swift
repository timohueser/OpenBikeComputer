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
    /// Every saved trip, newest first (TR6) — the top-level trip cards and the
    /// backing for the trip page. Read from the library (dangling stages already
    /// pruned) and refreshed on every trip edit.
    public private(set) var trips: [TripRecord] = []
    /// The interleaved Planned tab (TR6): trip cards + **loose** route cards (a
    /// filed route lives only inside its trip), newest first. Derived from
    /// `plannedRecords` + `trips`; `routes` stays the flat list of *all* planned
    /// summaries (filed included) so a stage still resolves to a detail.
    public private(set) var plannedItems: [PlannedItem] = []
    public private(set) var rides: [RideSummary] = []
    /// Recently Deleted (#292): trashed rides, most recently trashed first —
    /// the trash screen's rows and the Tracked tab's entry-row count.
    public private(set) var trashedRides: [RideSummary] = []
    public var tab: Tab = .planned
    public var searchText = ""
    /// #303: non-nil once a connected device reports an incompatible
    /// `protocol_version` — drives the incompatibility banner and disables sync.
    public private(set) var protocolMismatch: ProtocolMismatch?
    /// The connected device's `(serial, epoch)` identity (#769), established
    /// by each connection's `runIdentityCheck` and `nil` until it succeeds —
    /// **fail-closed**: a failed version+epoch read, a missing epoch (v1 peer,
    /// short read), or an empty serial leaves this `nil`, and with it
    /// `ackRides` and every reconcile write stay closed for the connection
    /// (library browsing is untouched). The gate re-opens on the next
    /// successful identity read. Every id-keyed write derives its scope from
    /// here: the possession ack filter, route-link minting on upload, the
    /// badge reconcile, and the legacy-claim migration.
    public private(set) var connectedScope: LibraryScope?
    /// Whether the connected device understands **auto-expiry** (epic #638) —
    /// settled by each connection's `setClock` in the prologue (`.stamped` → true,
    /// `.unsupported` → false; a thrown/absent stamp leaves the last verdict, so a
    /// flaky reconnect doesn't hide a known-capable device). Optimistic before the
    /// first stamp. **S7 hides the expiry UI behind this**, and the retention
    /// pushes are gated on it. A device predating expiry is a supported peer, not
    /// an error.
    public private(set) var supportsRetention = true
    /// Whether the identity read has settled this session (with an answer *or*
    /// a failed read) — the other half of the `canSync` gate, so an id-keyed
    /// write can never run ahead of the #303 verdict. See `runIdentityCheck`.
    @ObservationIgnored private var identityChecked = false

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

    /// The Planned tab rows after search (TR6) — trips and loose routes filtered
    /// by name together, so a query narrows the mixed list as one.
    public var filteredPlannedItems: [PlannedItem] {
        filtered(plannedItems, by: \.name)
    }

    public var filteredRides: [RideSummary] {
        filtered(rides, by: \.name)
    }

    // MARK: Wiring

    private let transport: any DeviceTransport
    private let library: any LibraryStore
    /// The app-local default-retention preference (epic #638) — read to seed a
    /// new upload's level and the upload sheet's picker, written by Settings.
    /// Never touched by a reconcile: changing the default is not a retro write.
    private let retentionDefaults: any RetentionDefaultsStore
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
    /// A reload requested while `loadTask` is already reading the catalogs.
    /// Store-change bursts set this bit instead of cancelling the live CoC
    /// download; the running task consumes it with one more reconcile pass.
    @ObservationIgnored private var reloadRequested = false
    /// The last route catalog a reload read — kept so `runIdentityCheck` can
    /// re-run the badge reconcile once the scope settles (#769): on launch the
    /// catalog read usually lands *before* the identity verdict, and clearing
    /// links under an unknown scope would be a reconcile write the fail-closed
    /// rule forbids.
    @ObservationIgnored private var lastRouteCatalog: [RouteCatalogEntry]?
    /// The connected device's per-object content CRCs — the v2 `routeList`
    /// `crc32` (spec §7.4), keyed by device object id. The **proof half** of the
    /// identity-verified badge (#770): a link is only a checkmark when this map
    /// holds a non-zero CRC for its object that equals the record's committed
    /// fingerprint. Rebuilt wholesale from every `listRoutes()` read; a
    /// just-committed upload pokes the one object it landed under (the transfer
    /// verified that CRC) so a fresh badge lights before the next catalog read.
    /// `0` (or an absent key) = unknown → proves nothing.
    @ObservationIgnored private var deviceRouteCRCs: [DeviceObjectID: UInt32] = [:]
    /// The last trip catalog a reload read (`tripList`) — the trip sibling of
    /// `lastRouteCatalog`, kept so the identity settle can re-run the trip
    /// reconcile once the scope is decidable, and so the whole-trip precheck can
    /// count device trip slots.
    @ObservationIgnored private var lastTripCatalog: [TripCatalogEntry]?
    /// The connected device's per-trip content CRCs — the v2 `tripList` `crc32`
    /// (spec §7.4), keyed by device trip id. The proof half of the trip badge
    /// (TR8, the route-CRC idiom, #770): a trip link is a checkmark only when this
    /// holds a non-zero CRC for its object equal to the record's committed
    /// fingerprint. Rebuilt wholesale from every `listTrips()` read; a
    /// just-committed trip upload pokes the one object it landed under.
    @ObservationIgnored private var deviceTripCRCs: [DeviceObjectID: UInt32] = [:]
    /// The #459 in-flight ledger the whole-trip upload sheet claims a token from
    /// (the same one the single-route sheet + ride sync use) — `nil` in tests /
    /// previews that don't exercise the lifecycle.
    @ObservationIgnored private let transferActivity: TransferActivity?
    /// The in-flight identity read (`runIdentityCheck`) for the current
    /// connection — what the coordinator's `identitySettled` seam awaits.
    /// Replaced (never cancelled) on a reconnect: a superseded read settles the
    /// same session-stable verdict on its own.
    @ObservationIgnored private var identityTask: Task<Void, Never>?

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
        retentionDefaults: any RetentionDefaultsStore = InMemoryRetentionDefaultsStore(),
        syncTiming: RideSyncCoordinator.Timing = RideSyncCoordinator.Timing(),
        nameReconciler: DeviceNameReconciler? = nil,
        transferActivity: TransferActivity? = nil,
        now: @escaping () -> Date = Date.init
    ) {
        self.transport = transport
        self.library = library
        self.retentionDefaults = retentionDefaults
        self.nameReconciler = nameReconciler
        self.transferActivity = transferActivity
        self.now = now
        self.sync = RideSyncCoordinator(
            transport: transport, library: library, timing: syncTiming,
            activity: transferActivity
        )
        // The coordinator's seams back into this model — weak, so the closures
        // the model's own coordinator holds can never pin the model.
        // Closed — not open — until the identity read settles: neither the SYNC
        // decode path nor the possession ack may run ahead of the #303 verdict.
        // The settle seam is what makes an early SYNC tap wait for that verdict
        // instead of hitting the closed gate and no-oping.
        //
        // v2 hardening (#769): the verdict must also have produced a scope —
        // a *failed* identity read (or one without an epoch) keeps the gate
        // CLOSED, where #764's v1 posture settled it open. Fail-open would
        // let a sync persist id-keyed state under an unknown era, re-creating
        // the 2026-07-12 incident in the failure path.
        sync.canSync = { [weak self] in
            guard let self else { return false }
            return identityChecked && protocolMismatch == nil && connectedScope != nil
        }
        sync.identitySettled = { [weak self] in await self?.identityTask?.value }
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
        reloadTrips()
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
                    // Re-run the identity read per connection — the verdict can
                    // genuinely change between connects (a DFU install), and
                    // its completion is what re-fires the possession ack.
                    identityTask = Task { [weak self] in
                        await self?.runIdentityCheck()
                    }
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
        streamTasks.append(Task { [weak self, transport] in
            for await change in transport.storeChanges {
                guard let self else { return }
                // The device's store moved under an open app — an on-device
                // route delete (epic #447 P6) or an upload committed from
                // elsewhere. Re-read + reconcile so the "on device" badge
                // clears (and a re-upload is offered) without a reconnect.
                // Rides move only through Sync, so only route/trip movements
                // trigger the reload (a device-side route or trip delete, TR3's
                // long-press cascade). A burst coalesces behind the in-flight
                // read; an opened CoC exchange is never cancelled halfway.
                if change.type == .route || change.type == .trip { reload() }
            }
        })
        // Notifications are deliberately best-effort on BLE. Keep them as the
        // immediate path, then audit the tiny route/trip catalogs occasionally
        // so a dropped edge cannot leave an "on device" checkmark stale until
        // the next app launch or reconnect.
        streamTasks.append(Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                guard !Task.isCancelled, let self else { return }
                if connection == .connected { reload() }
            }
        })
        reload()
        // Identity after the first library read: a fault armed for "the first
        // read" (the S3 scenario) must hit the lists, not this fetch. The same
        // read carries the protocol-version check (#303) — surfaced here, on
        // connect, where `deviceInfo()` is consumed.
        let firstLoad = loadTask
        identityTask = Task { [weak self] in
            await firstLoad?.value
            await self?.runIdentityCheck()
        }
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
        identityTask?.cancel()
    }

    /// (Re)read both lists — also the S3 "Retry" action. Cached content stays
    /// up while the fresh read runs; only an *empty* library shows skeletons.
    public func reload() {
        // An incompatible device (#303): don't decode its objects — keep the
        // library-first content up and let the banner explain. The first load
        // (before the version is read) may still run; every reload after the
        // mismatch is known is gated here.
        guard protocolMismatch == nil else {
            reloadRequested = false
            loadState = .loaded
            return
        }
        reloadRequested = true
        // Never cancel a list transfer after its descriptor has opened the raw
        // CoC exchange. The next requested pass runs as soon as this one closes.
        guard loadTask == nil else { return }
        loadState = .loading
        loadTask = Task { [weak self] in
            await self?.runReloadLoop()
        }
    }

    /// Drain coalesced catalog requests serially. This method is main-actor
    /// isolated, so clearing the dirty bit and retiring `loadTask` cannot race a
    /// `storeChanged` callback that asks for another pass.
    private func runReloadLoop() async {
        while reloadRequested, !Task.isCancelled {
            reloadRequested = false
            do {
                // Only the route catalog is read here: Planned reconciles its
                // on-device badges against it, and Tracked is library-first
                // (#296) so its rows come from the local library — the device's
                // rides are pulled only by Sync, never on a plain (re)load.
                let deviceRoutes = try await transport.listRoutes()
                guard !Task.isCancelled else { break }
                lastRouteCatalog = deviceRoutes
                reconcileOnDevice(with: deviceRoutes)
                // The trip catalog rides the same reload (TR8): reconcile each
                // trip's device link/badge against `tripList`. Best-effort, but
                // fail-CLOSED: a failed read skips the reconcile entirely (stale
                // links beat nuked ones) — treating a transient `listTrips`
                // failure as "zero trips" dropped every trip link, and the next
                // "Upload trip" then minted a duplicate device trip instead of
                // replacing in place. A device predating trips rejects the read
                // (`notFound`), lands here too, and has no trip links to keep —
                // the old-firmware posture is unchanged.
                if let deviceTrips = try? await transport.listTrips() {
                    guard !Task.isCancelled else { break }
                    lastTripCatalog = deviceTrips
                    reconcileTripsOnDevice(with: deviceTrips)
                }
                routes = plannedList()
                reloadTrips()
                rides = trackedList()
                loadState = .loaded
            } catch {
                guard !Task.isCancelled else { break }
                loadState = .failed
            }
        }
        loadTask = nil
    }

    /// One identity read (`deviceInfo()`) for the current connection: the #303
    /// protocol-version verdict **and the (serial, epoch) scope** (#769), and —
    /// strictly downstream of both — the legacy-claim migration and the
    /// coordinator's possession ack, so an id-keyed write can never race the
    /// verdict or run under an unknown era.
    ///
    /// **Fail-closed (#769, reversing #764's v1 posture):** "settled" still
    /// includes a *failed* read (launched offline, a flaky link) — the SYNC
    /// button stops waiting — but the gate stays **closed**: no scope means no
    /// `ackRides` and no reconcile writes for this connection. Library
    /// browsing is unaffected, and the next connect edge re-runs the check
    /// (the gate re-opens on the first successful read). A compatible read
    /// clears a stale mismatch (a DFU install can fix the device between
    /// connects).
    private func runIdentityCheck() async {
        // Unknown until proven, every connection: the device may have been
        // wiped (new epoch) or swapped since the last read.
        connectedScope = nil
        // Stamp the device's trusted wall clock (epic #638) on **every connect,
        // before the first ack / reconcile write** (spec §4.4): the sweep's ride
        // `synced_at` stamping assumes a trusted clock, and this is what
        // establishes it. Also settles `supportsRetention` for the connection.
        await stampDeviceClock()
        if let info = try? await transport.deviceInfo() {
            deviceName = info.name
            if case let .protocolMismatch(expected, found)? =
                OBCProtocol.versionMismatch(reportedBy: info.protocolVersion) {
                protocolMismatch = ProtocolMismatch(expected: expected, found: found)
            } else {
                protocolMismatch = nil
                // `libraryScope` is nil on a missing epoch or empty serial —
                // the fail-closed input, never defaulted (`0` is a legal epoch).
                connectedScope = info.libraryScope
            }
        }
        // The verdict is in (scope included) — the `canSync` gate may answer.
        // Early SYNC taps still wait out the rest of this task: the
        // `identitySettled` seam awaits the whole task, claim included, so a
        // sync can never race the migration's re-keys.
        identityChecked = true
        if let scope = connectedScope {
            // One-time v1 → scoped migration, claim-on-first-contact (#769):
            // runs before the ack so freshly-claimed ids are acked (and before
            // any sync — see above — so a corroborated flat ride can't
            // re-download as "new" under its scoped key, which would be the
            // duplicate row the claim forbids).
            await claimLegacyLibraryEntries(for: scope)
            // The per-connect possession ack (spec §4.4), scope-filtered — a
            // send that misses a dying link is dropped and covered by the next
            // connect's re-ack.
            sync.reconcilePossession(for: scope)
            // Route + trip links could not be reconciled while the scope was
            // unknown (reload may have run first) — true them up against the
            // cached catalogs now that their validity is decidable.
            if let catalog = lastRouteCatalog {
                reconcileOnDevice(with: catalog)
                routes = plannedList()
            }
            if let tripCatalog = lastTripCatalog {
                reconcileTripsOnDevice(with: tripCatalog)
            }
            reloadTrips()
        }
    }

    /// Run one claim pass of the v1 → scoped migration against the connected
    /// device (see `LibraryScopeMigrator`). Skipped in one cheap check once no
    /// flat legacy state remains — the steady state costs no device read. The
    /// ride-list read is the claim's corroboration evidence; if it fails, the
    /// pass simply waits for the next connect (nothing is guessed).
    private func claimLegacyLibraryEntries(for scope: LibraryScope) async {
        guard LibraryScopeMigrator.hasLegacyState(in: library) else { return }
        guard let catalog = try? await transport.listRides() else { return }
        LibraryScopeMigrator.run(in: library, scope: scope, deviceRides: catalog.rides)
        // Ids may have moved under the claim — re-read every mirror the lists
        // are built from (the same set `start()` seeds).
        rideSummaries = Dictionary(
            uniqueKeysWithValues: library.rideSummaries().map { ($0.id, $0) })
        deletedRideIDs = library.deletedRideIDs()
        trashedRideIDs = library.trashedRideIDs()
        rides = trackedList()
        trashedRides = trashedList()
    }

    /// Stamp the device's trusted wall clock (`setClock`, epic #638) with the
    /// phone's current time + local UTC offset, and settle `supportsRetention` for
    /// the connection. `.unsupported` (a device predating expiry) is a supported
    /// peer — no error surfaced. A thrown/absent stamp (a link dying mid-prologue)
    /// leaves the last verdict untouched: the next connect re-stamps, and a flaky
    /// reconnect must not flip a known-capable device to "hidden".
    private func stampDeviceClock() async {
        guard let outcome = try? await transport.setClock(WallClockSample()) else { return }
        supportsRetention = (outcome == .stamped)
    }

    /// The reconcile half of route retention (epic #638): land the device's
    /// reported `expires_at`/`retention` on each linked record (display-only) and
    /// push the desired level when it diverges. Capability-gated — a device
    /// predating expiry reports `nil` retention, so nothing pushes — and a `nil`
    /// **desired** level never pushes (invariant 6: a route uploaded before this
    /// feature migrates as "not set" and can't be surprise-deleted). Scope-gated
    /// via the same valid-link predicate the badge reconcile uses.
    private func reconcileRetention(scope: LibraryScope, catalog: [RouteCatalogEntry]) {
        let byID = Dictionary(catalog.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for (id, record) in plannedRecords {
            guard let link = record.deviceLink, link.matches(scope),
                let entry = byID[link.objectID] else { continue }
            var updated = record
            // Device truth, refreshed wholesale from the catalog (display-only).
            updated.deviceExpiresAt = entry.expiresAt
            updated.deviceRetention = entry.retention
            // Push the desired level when it's set and diverges from the device's.
            // `entry.retention != nil` is the capability signal (a pre-expiry
            // device reports no retention), belt-and-braces with the flag.
            if supportsRetention, let desired = record.retention,
                entry.retention != nil, desired != entry.retention {
                pushRetention(desired, to: link.objectID)
                updated.deviceRetention = desired  // optimistic; a later list confirms
            }
            if updated != record {
                plannedRecords[id] = updated
                library.savePlannedRoute(updated)
            }
        }
    }

    /// Fire-and-forget `setRouteRetention` (epic #638) — best-effort like the
    /// possession ack / name reconcile: a failed push self-heals at the next
    /// reconcile (the desired level still diverges) or on reconnect, so the error
    /// is dropped rather than surfaced. Captures only the transport.
    private func pushRetention(_ retention: Retention, to objectID: DeviceObjectID) {
        Task { [transport] in
            _ = try? await transport.setRouteRetention(objectID, retention)
        }
    }

    /// True-up every record's `deviceLink` against the device's live catalog
    /// (device object ids **and** content CRCs — #770), then adopt-by-content.
    /// Absence, or a catalog CRC that disagrees with what we committed, drops
    /// the link (a copy deleted out from under us, era aliasing that survived
    /// scoping, or an on-device replacement by another phone); an *unlinked*
    /// catalog entry whose CRC matches a record's current encoding re-links it.
    ///
    /// Scope-gated both ways (#769): with the identity **unknown** no link is
    /// written at all (fail-closed — a catalog can't be attributed to a scope
    /// that hasn't been proven), and with it known only links that **match the
    /// connected scope** are eligible to clear — device B's catalog says
    /// nothing about the copies device A legitimately holds (the v1
    /// link-clearing bug this issue retires). The catalog CRCs are cached either
    /// way so a later identity settle can prove the badge against them.
    private func reconcileOnDevice(with deviceRoutes: [RouteCatalogEntry]) {
        // The proof half of the badge (#770) — refreshed wholesale from device
        // truth on every read, replacing any optimistic post-upload pokes.
        deviceRouteCRCs = Dictionary(
            deviceRoutes.map { ($0.id, $0.crc32) }, uniquingKeysWith: { first, _ in first })
        guard let scope = connectedScope else {
            refreshOnDeviceStates()
            return
        }
        let listed = Set(deviceRoutes.map(\.id))
        // 1) Drop links the catalog *disproves*. Absent object → gone. Present
        //    object whose non-zero CRC differs from our committed fingerprint →
        //    it holds different content than we think (aliasing / foreign
        //    replacement) — never a checkmark on presence. A `crc32 = 0`
        //    (unknown) entry proves nothing, so the link is kept conservatively.
        for (id, var record) in plannedRecords {
            guard let link = record.deviceLink, link.matches(scope) else { continue }
            let present = listed.contains(link.objectID)
            let catalogCRC = deviceRouteCRCs[link.objectID] ?? 0
            let crcMismatch = present && catalogCRC != 0
                && record.uploadedCRC32 != nil && catalogCRC != record.uploadedCRC32
            guard !present || crcMismatch else { continue }
            record.deviceLink = nil
            record.uploadedCRC32 = nil
            plannedRecords[id] = record
            library.savePlannedRoute(record)
        }
        // 2) Adopt-by-content — heal identical unlinked copies (app reinstall,
        //    device switch-back) without a re-upload.
        adoptByContent(scope: scope, catalog: deviceRoutes)
        // 3) Retention (epic #638): land the device's expiry truth on each linked
        //    record and push the desired level where it diverges.
        reconcileRetention(scope: scope, catalog: deviceRoutes)
        refreshOnDeviceStates()
    }

    /// Adopt-by-content (#770): an **unlinked** catalog entry whose non-zero
    /// `crc32` equals a record's *current* OBCR encoding re-links to it (the
    /// badge lights, no upload needed), and a subsequent upload replaces that
    /// object by id instead of creating a duplicate. Heals the app-reinstall
    /// (link lost, device kept) and device-switch-back cases silently.
    ///
    /// Ambiguity is resolved first-come, each side claimed at most once: two
    /// catalog entries with the same CRC → the first (device order) is adopted,
    /// the rest left; two records with the same current CRC → the first (stable
    /// id order) adopts the entry, the rest stay unlinked. A later delete on
    /// either side reconciles normally.
    private func adoptByContent(scope: LibraryScope, catalog: [RouteCatalogEntry]) {
        // Object ids already spoken for by a valid link — never adopt over them.
        var claimed = Set(plannedRecords.values.compactMap { record -> DeviceObjectID? in
            guard let link = record.deviceLink, link.matches(scope) else { return nil }
            return link.objectID
        })
        // Adoptable = listed entries with a known (non-zero) CRC not already
        // claimed. Device order is preserved, so the "adopt the first" tie-break
        // falls out of the scan below.
        let adoptable = catalog.filter { $0.crc32 != 0 && !claimed.contains($0.id) }
        guard !adoptable.isEmpty else { return }
        // Records with no *valid* link, in a deterministic (stable id) order so
        // an adoption is reproducible run to run.
        let candidates = plannedRecords.values
            .filter { record in
                guard let link = record.deviceLink else { return true }
                return !link.matches(scope)
            }
            .sorted { $0.id.rawValue < $1.id.rawValue }
        for record in candidates {
            let currentCRC = RouteObjectCodec.payloadCRC(for: record)
            guard let entry = adoptable.first(where: {
                $0.crc32 == currentCRC && !claimed.contains($0.id)
            }) else { continue }
            var adopted = record
            adopted.deviceLink = DeviceRouteLink(
                serial: scope.serial, epoch: scope.epoch, objectID: entry.id)
            adopted.uploadedCRC32 = currentCRC
            plannedRecords[record.id] = adopted
            library.savePlannedRoute(adopted)
            claimed.insert(entry.id)
        }
    }

    /// The CRC the connected device is **proven** to currently hold for this
    /// record, or `nil` when unproven (#770). Proof = a link valid for the
    /// connected scope + a non-zero catalog CRC for that object that equals the
    /// record's committed fingerprint. An unknown catalog CRC (`0`), a missing
    /// fingerprint, or a mismatch all read as unproven — no badge.
    private func provenCommittedCRC(for record: PlannedRouteRecord) -> UInt32? {
        guard let scope = connectedScope, let link = record.deviceLink,
            link.matches(scope), let uploaded = record.uploadedCRC32,
            let catalogCRC = deviceRouteCRCs[link.objectID], catalogCRC != 0,
            catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    /// Recompute every record's proven device-copy state — called whenever a
    /// record's content or its device link moves (load, reconcile, upload,
    /// rename, re-import, delete). The payload encode behind the CRC only runs
    /// for records the device is *proven* to hold (an unproven record short-
    /// circuits to `.notOnDevice` before the encode).
    private func refreshOnDeviceStates() {
        onDevice = plannedRecords.mapValues { record in
            OnDeviceState.determine(
                provenCommittedCRC: provenCommittedCRC(for: record),
                currentCRC: { RouteObjectCodec.payloadCRC(for: record) }
            )
        }
    }

    /// The Planned rows: the library's records, newest first.
    private func plannedList() -> [RouteSummary] {
        plannedRecords.values.sorted { $0.addedAt > $1.addedAt }.map(\.summary)
    }

    // MARK: Trips (route grouping, TR6)

    /// Re-read trips from the library (dangling stages pruned there) and rebuild
    /// the interleaved Planned list — the one call every trip/route edit ends on.
    /// Internal (not private) so `@testable` tests can re-sync the model's trips
    /// after mutating the library directly.
    func reloadTrips() {
        trips = library.trips()
        rebuildPlannedItems()
    }

    /// Recompute the interleaved Planned tab from the current records + trips.
    private func rebuildPlannedItems() {
        plannedItems = PlannedItem.partition(records: Array(plannedRecords.values), trips: trips)
    }

    /// The trip behind `id`, or `nil` if it dissolved / never existed.
    public func trip(_ id: TripID) -> TripRecord? { trips.first { $0.id == id } }

    /// The existing trips as picker rows (TR7) — the one projection the shared
    /// `TripPickerSheet` reads for the import row and the route menus. Newest
    /// first, matching the top-level list order.
    public var tripPickerItems: [TripPickerItem] {
        trips.map { TripPickerItem(id: $0.id, name: $0.name, stageCount: $0.stageIDs.count) }
    }

    /// The trip a route is currently filed in, or `nil` when it's loose (TR7) —
    /// drives the route menu's Add-vs-Move/Remove split and the picker's
    /// current-trip checkmark.
    public func tripContaining(_ routeID: RouteID) -> TripID? {
        trips.first { $0.stageIDs.contains(routeID) }?.id
    }

    /// A trip's member routes as list summaries, **in ride order** — the trip
    /// page's rows. Skips any stage whose record is gone (already pruned on read;
    /// belt-and-suspenders here).
    public func tripStages(_ id: TripID) -> [RouteSummary] {
        guard let trip = trip(id) else { return [] }
        return trip.stageIDs.compactMap { plannedRecords[$0]?.summary }
    }

    /// A trip's summed stats (distance/climb + stage count) — the single
    /// definition, `TripStats.summing` over the resolved member summaries.
    public func tripStats(_ id: TripID) -> TripStats {
        TripStats.summing(tripStages(id))
    }

    /// The trip-level device-copy state behind the card badge (TR6/TR8): the trip
    /// is "up to date" only when the **trip object itself** is proven current
    /// *and* every stage is up to date. The trip object's own proof (TR8) is a
    /// valid scoped link plus a non-zero `tripList` `crc32` that equals the
    /// committed fingerprint — the same route-CRC idiom. A trip the phone never
    /// pushed (no link) reads `.notOnDevice`.
    public func tripOnDeviceState(_ id: TripID) -> OnDeviceState {
        guard let trip = trip(id), !trip.stageIDs.isEmpty else { return .notOnDevice }
        let tripSelf = OnDeviceState.determine(
            provenCommittedCRC: provenTripCommittedCRC(for: trip),
            currentCRC: { currentTripPayloadCRC(for: trip) }
        )
        let stageStates = trip.stageIDs.map { onDeviceState($0) }
        return Self.composeTripState(tripSelf: tripSelf, stageStates: stageStates)
    }

    /// The device stage ids a trip object upload would carry — each stage's
    /// **committed** device object id (a valid scoped link), in ride order,
    /// dropping any stage not currently on the device. This is the compaction the
    /// stages-first-trip-last upload relies on; the one definition the encode, the
    /// fingerprint, and the plan all read.
    private func currentTripDeviceStageIDs(for trip: TripRecord) -> [DeviceObjectID] {
        trip.stageIDs.compactMap { plannedDeviceObjectID(for: $0) }
    }

    /// The CRC-32 of the trip object an upload of this trip would send now — its
    /// name + resolved stage ids. The trip-level `OnDeviceState` fingerprint.
    private func currentTripPayloadCRC(for trip: TripRecord) -> UInt32 {
        TripObjectCodec.payloadCRC(name: trip.name, deviceStageIDs: currentTripDeviceStageIDs(for: trip))
    }

    /// The CRC the connected device is **proven** to currently hold for this trip
    /// (TR8), or `nil` when unproven — a valid scoped link, a non-zero catalog
    /// CRC for that object, and equality with the committed fingerprint. Mirrors
    /// `provenCommittedCRC(for:)` for routes exactly.
    private func provenTripCommittedCRC(for trip: TripRecord) -> UInt32? {
        guard let scope = connectedScope, let link = trip.deviceLink, link.matches(scope),
            let uploaded = trip.uploadedCRC32,
            let catalogCRC = deviceTripCRCs[link.objectID], catalogCRC != 0,
            catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    /// True-up every trip's `deviceLink` against the device's live `tripList`
    /// (TR8) — the trip sibling of `reconcileOnDevice`, drop pass **and** adopt
    /// pass. A device-side trip delete (absent object) or a foreign replacement
    /// (a present object whose non-zero CRC disagrees with what we committed)
    /// drops the link; a local edit (rename, reorder) never does — it leaves the
    /// committed CRC intact and reads as outdated through `currentTripPayloadCRC`.
    /// Then an *unlinked* catalog entry whose CRC matches a trip's current
    /// encoding re-links it — the trip twin of `adoptByContent` (#770), and the
    /// heal for the lost-ack fresh upload (the trip object landed, the commit ack
    /// didn't): without it the retry planned a fresh trip and minted a same-name
    /// twin folder on the device. Scope-gated both ways (#769): unknown scope
    /// writes nothing, known scope clears only links that match it.
    private func reconcileTripsOnDevice(with catalog: [TripCatalogEntry]) {
        deviceTripCRCs = Dictionary(
            catalog.map { ($0.id, $0.crc32) }, uniquingKeysWith: { first, _ in first })
        guard let scope = connectedScope else { return }
        let listed = Set(catalog.map(\.id))
        for var trip in library.trips() {
            guard let link = trip.deviceLink, link.matches(scope) else { continue }
            let present = listed.contains(link.objectID)
            let catalogCRC = deviceTripCRCs[link.objectID] ?? 0
            let crcMismatch = present && catalogCRC != 0
                && trip.uploadedCRC32 != nil && catalogCRC != trip.uploadedCRC32
            guard !present || crcMismatch else { continue }
            trip.deviceLink = nil
            trip.uploadedCRC32 = nil
            library.saveTrip(trip)
        }
        adoptTripsByContent(scope: scope, catalog: catalog)
    }

    /// Adopt-by-content for trips — `adoptByContent`'s trip twin, with the same
    /// tie-breaks (each side claimed at most once, deterministic order). The
    /// fingerprint is the trip's *current* encoding (`currentTripPayloadCRC`:
    /// name + committed device stage ids), so this runs after the **route**
    /// reconcile has trued the stage links up — both call sites order it so.
    private func adoptTripsByContent(scope: LibraryScope, catalog: [TripCatalogEntry]) {
        var claimed = Set(library.trips().compactMap { trip -> DeviceObjectID? in
            guard let link = trip.deviceLink, link.matches(scope) else { return nil }
            return link.objectID
        })
        let adoptable = catalog.filter { $0.crc32 != 0 && !claimed.contains($0.id) }
        guard !adoptable.isEmpty else { return }
        let candidates = library.trips()
            .filter { trip in
                guard let link = trip.deviceLink else { return true }
                return !link.matches(scope)
            }
            .sorted { $0.id.rawValue < $1.id.rawValue }
        for var trip in candidates {
            let currentCRC = currentTripPayloadCRC(for: trip)
            guard let entry = adoptable.first(where: {
                $0.crc32 == currentCRC && !claimed.contains($0.id)
            }) else { continue }
            trip.deviceLink = DeviceRouteLink(
                serial: scope.serial, epoch: scope.epoch, objectID: entry.id)
            trip.uploadedCRC32 = currentCRC
            library.saveTrip(trip)
            claimed.insert(entry.id)
        }
    }

    // MARK: Whole-trip upload (TR8)

    /// Build the whole-trip upload plan (TR8) — partition the stages into skip /
    /// replace / fresh and do the precheck math (`TripUploadPlanner`). `nil` when
    /// the trip has dissolved. The device counts come from the last reconcile's
    /// catalogs; the trip-object action replaces its existing device copy when a
    /// valid scoped link points at a still-present `tripList` entry, else fresh.
    public func planTripUpload(_ id: TripID) -> TripUploadPlan? {
        guard let trip = trip(id) else { return nil }
        let stageInputs = trip.stageIDs.map { routeID in
            TripUploadPlanner.StageInput(
                routeID: routeID,
                isUpToDate: onDeviceState(routeID) == .upToDate,
                committedObjectID: plannedDeviceObjectID(for: routeID)
            )
        }
        // A valid scoped link IS the replace target — exactly `plannedDeviceObjectID`'s
        // rule for route stages, no catalog-contains check. The reconcile owns dropping
        // links for trips the device no longer lists (every successful `tripList` read);
        // re-checking a *cached* catalog here demoted a valid link to a fresh upload
        // whenever the cache was stale (e.g. the post-commit `listTrips` failed) — and a
        // fresh upload of an already-stored trip mints a silent duplicate. A replace of
        // a genuinely vanished trip fails loudly (`notFound`) instead — the safe side.
        let tripObjectID: DeviceObjectID? = {
            guard let link = trip.deviceLink, let scope = connectedScope, link.matches(scope)
            else { return nil }
            return link.objectID
        }()
        return TripUploadPlanner.plan(
            stages: stageInputs,
            tripObjectID: tripObjectID,
            deviceRouteCount: lastRouteCatalog?.count ?? 0,
            deviceTripCount: lastTripCatalog?.count ?? 0
        )
    }

    /// Re-read both device catalogs and reconcile (adoption included) **before**
    /// planning a whole-trip upload — the retry-after-a-failure path. A plan cut
    /// from catalogs cached before the failure can't see what actually landed:
    /// a stage (or the trip object) that committed but whose ack was lost would
    /// re-plan as *fresh* and mint a device twin. The fresh read lets the
    /// reconcile adopt those orphans by content first, so the retry plans skips
    /// and replaces instead. Either read failing falls back to the cached
    /// catalogs (the device-side fresh-upload dedup, spec §4.2, still backstops
    /// convergence); order matters — routes before trips, the adoption rule's
    /// dependency.
    public func prepareTripUpload(
        _ id: TripID, timing: TripUploadModel.Timing = TripUploadModel.Timing()
    ) async -> TripUploadModel? {
        if connection == .connected {
            if let deviceRoutes = try? await transport.listRoutes() {
                lastRouteCatalog = deviceRoutes
                reconcileOnDevice(with: deviceRoutes)
                routes = plannedList()
            }
            if let deviceTrips = try? await transport.listTrips() {
                lastTripCatalog = deviceTrips
                reconcileTripsOnDevice(with: deviceTrips)
            }
            reloadTrips()
        }
        return makeTripUploadModel(id, timing: timing)
    }

    /// The whole-trip upload sheet's driver (TR8), or `nil` when the trip
    /// dissolved. Turns the plan into the queue: a step per stage (skip / upload)
    /// in ride order, then the trip object **last** — unless everything's already
    /// current (all stages up to date *and* the trip object proven), in which case
    /// the queue is pure skips and nothing is sent. Each step commits its object's
    /// link the instant it lands, via `markRouteUploaded` / `markTripUploaded`.
    public func makeTripUploadModel(
        _ id: TripID, timing: TripUploadModel.Timing = TripUploadModel.Timing()
    ) -> TripUploadModel? {
        guard let trip = trip(id), let plan = planTripUpload(id) else { return nil }
        var steps: [TripUploadModel.QueueStep] = []
        for stagePlan in plan.stages {
            let routeID = stagePlan.routeID
            let name = plannedRecords[routeID]?.summary.name ?? "Stage"
            switch stagePlan.action {
            case .skip:
                steps.append(.skip(title: name))
            case .fresh, .replace:
                let target: DeviceObjectID? =
                    if case .replace(let objectID) = stagePlan.action { objectID } else { nil }
                steps.append(.transfer(
                    title: name,
                    makeTransfer: { [weak self] in
                        guard let self, let blob = self.makeStageBlob(routeID, target: target) else { return nil }
                        return (self.transport.uploadRoute(blob), CRC32.checksum(blob.payload))
                    },
                    commit: { [weak self] objectID, crc in
                        guard let objectID else { return }
                        // adopt: false — the queue pushes the trip object itself, last.
                        // No per-stage retention choice (no upload sheet): the
                        // stage takes its existing level or the app default.
                        self?.markRouteUploaded(
                            routeID, objectID: objectID, crc32: crc, retention: nil, adopt: false)
                    }
                ))
            }
        }
        // The trip object, last — skipped only when every stage is current *and*
        // the trip object itself is proven up to date (nothing to push).
        let tripProven = provenTripCommittedCRC(for: trip)
        let tripObjectUpToDate = tripProven != nil && tripProven == currentTripPayloadCRC(for: trip)
        if !(plan.allStagesSkip && tripObjectUpToDate) {
            let target: DeviceObjectID? =
                if case .replace(let objectID) = plan.tripObject { objectID } else { nil }
            steps.append(.transfer(
                title: "Trip details",
                makeTransfer: { [weak self] in
                    guard let self, let blob = self.makeTripBlob(id, target: target) else { return nil }
                    return (self.transport.uploadTrip(blob), CRC32.checksum(blob.payload))
                },
                commit: { [weak self] objectID, crc in
                    self?.markTripUploaded(id, objectID: objectID, crc32: crc)
                }
            ))
        }
        return TripUploadModel(
            transport: transport, tripName: trip.name, deviceName: deviceName,
            precheck: plan.precheck, steps: steps, timing: timing, activity: transferActivity
        )
    }

    /// The `RouteBlob` a stage upload sends — the same OBCR v2 payload a single
    /// route upload builds (`RouteObjectCodec`), under the given replace target.
    private func makeStageBlob(_ routeID: RouteID, target: DeviceObjectID?) -> RouteBlob? {
        guard let record = plannedRecords[routeID] else { return nil }
        let payload = RouteObjectCodec.encode(
            points: record.route.points, waypoints: record.route.waypoints, name: record.summary.name)
        guard !payload.isEmpty else { return nil }
        return RouteBlob(
            summary: record.summary, waypoints: record.route.waypoints,
            payload: payload, targetObjectID: target)
    }

    /// The `TripBlob` the trip object upload sends — encoded from the trip's name
    /// + its **currently resolvable** device stage ids (built at execution time,
    /// after the stages committed), under the given replace target. `nil` when no
    /// stage resolves to a device copy (nothing to reference).
    private func makeTripBlob(_ tripID: TripID, target: DeviceObjectID?) -> TripBlob? {
        guard let trip = trip(tripID) else { return nil }
        let deviceStageIDs = currentTripDeviceStageIDs(for: trip)
        guard !deviceStageIDs.isEmpty else { return nil }
        let payload = TripObjectCodec.encode(name: trip.name, deviceStageIDs: deviceStageIDs)
        return TripBlob(
            name: trip.name, deviceStageIDs: deviceStageIDs, payload: payload, targetObjectID: target)
    }

    /// Compose the trip badge from the trip object's own state and its stages'
    /// (TR6). Pure + `static` so the rule is unit-testable without a device.
    static func composeTripState(
        tripSelf: OnDeviceState, stageStates: [OnDeviceState]
    ) -> OnDeviceState {
        guard !stageStates.isEmpty, tripSelf != .notOnDevice else { return .notOnDevice }
        if tripSelf == .upToDate, stageStates.allSatisfy({ $0 == .upToDate }) { return .upToDate }
        return .outdated
    }

    /// Rename a trip (H12 idiom) — phone-local, persisted; the new name rides the
    /// next trip upload (TR8). A no-op name is the caller's guard.
    public func renameTrip(_ id: TripID, to name: String) {
        guard var trip = trip(id) else { return }
        trip.name = name
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Reorder a trip's stages (drag) — ride order is the trip's source of truth,
    /// so this is the whole edit; a reorder out-dates the device copy (TR8).
    public func reorderTripStages(_ id: TripID, from source: IndexSet, to destination: Int) {
        guard var trip = trip(id) else { return }
        trip.stageIDs.move(fromOffsets: source, toOffset: destination)
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Remove one stage from a trip — the route returns to the top level (its
    /// record is untouched). Removing the **last** stage dissolves the trip;
    /// returns `true` in that case so the caller can pop the page.
    @discardableResult
    public func removeStage(_ routeID: RouteID, from tripID: TripID) -> Bool {
        guard var trip = trip(tripID) else { return false }
        trip.stageIDs.removeAll { $0 == routeID }
        if trip.stageIDs.isEmpty {
            library.deleteTrip(tripID)  // dissolve — routes stay in the library
            reloadTrips()
            return true
        }
        library.saveTrip(trip)
        reloadTrips()
        return false
    }

    /// **Ungroup** a trip (the Delete dialog's non-destructive branch): drop the
    /// trip metadata; every member route stays in the library and returns to the
    /// top level. Routes are untouched — the store's `deleteTrip` contract.
    public func ungroupTrip(_ id: TripID) {
        library.deleteTrip(id)
        reloadTrips()
    }

    /// **Delete trip & routes** (the Delete dialog's destructive branch): the
    /// cascade the initiating UI composes (the protocol-level trip delete is
    /// non-cascading) — while connected, delete each member route's device copy
    /// **and** the trip object, then the phone library. Offline, only the phone
    /// copies go; the device copies surface as orphans at the next reconcile,
    /// exactly like a deleted route today (H1).
    public func deleteTripAndRoutes(_ id: TripID) {
        guard let trip = trip(id) else { return }
        let stages = trip.stageIDs
        // Device-side cascade (composed here — the protocol trip delete is
        // non-cascading): scope-gated per-route deletes + the trip delete,
        // best-effort. A failed command leaves an orphan reconcile heals.
        if let scope = connectedScope {
            let routeObjectIDs: [DeviceObjectID] = stages.compactMap { stage in
                guard let link = plannedRecords[stage]?.deviceLink, link.matches(scope) else { return nil }
                return link.objectID
            }
            let tripObjectID: DeviceObjectID? = {
                guard let link = trip.deviceLink, link.matches(scope) else { return nil }
                return link.objectID
            }()
            if !routeObjectIDs.isEmpty || tripObjectID != nil {
                Task { [transport] in
                    for objectID in routeObjectIDs { try? await transport.deleteRoute(objectID) }
                    if let tripObjectID { try? await transport.deleteTrip(tripObjectID) }
                }
            }
        }
        library.deleteTrip(id)
        for stage in stages {
            routes.removeAll { $0.id == stage }
            plannedRecords[stage] = nil
            onDevice[stage] = nil
            library.deletePlannedRoute(stage)
        }
        reloadTrips()
    }

    // MARK: Create & file (TR7)

    /// **Group** the selected routes into a new trip (the multi-select retrofit
    /// path). Selection order is *not* stage order — stages default to the
    /// routes **as listed in the Planned list** (newest `addedAt` first, the same
    /// order the loose cards show), and reordering stays the trip page's job. The
    /// new trip takes the slot of its newest member so its card appears in place.
    /// Ids with no live record are dropped; an empty result creates nothing (no
    /// empty trips). Returns the new trip's id.
    @discardableResult
    public func groupIntoTrip(_ routeIDs: [RouteID], name: String) -> TripID? {
        let ordered = routeIDs
            .compactMap { plannedRecords[$0] }
            .sorted { $0.addedAt > $1.addedAt }
        guard !ordered.isEmpty else { return nil }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let trip = TripRecord(
            id: TripID(UUID().uuidString.lowercased()),
            name: trimmed.isEmpty ? "New trip" : trimmed,
            stageIDs: ordered.map(\.id),
            // In place: sit where the newest grouped route sat.
            addedAt: ordered.map(\.addedAt).max() ?? now()
        )
        library.saveTrip(trip)  // ≤ 1-trip invariant enforced in the store
        reloadTrips()
        return trip.id
    }

    /// File a route per a picker `TripSelection` (TR7) — the one call the import
    /// row and the route menus' Add/Move both end on. `.existing` appends the
    /// route as the trip's **last stage** (the store's invariant strips it from
    /// any other trip, so a move is an implicit remove); `.new` starts a trip
    /// with it as the first stage, in that route's list slot; `.none` files
    /// nothing (the import row's opt-out). Phone-local, offline-safe — library
    /// writes only (device adoption is TR8's).
    public func fileRoute(_ routeID: RouteID, into selection: TripSelection) {
        switch selection {
        case .none:
            break
        case .existing(let tripID):
            guard var trip = trip(tripID), !trip.stageIDs.contains(routeID) else { return }
            trip.stageIDs.append(routeID)  // last stage
            library.saveTrip(trip)
            reloadTrips()
        case .new(let name):
            let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
            guard plannedRecords[routeID] != nil else { return }
            let trip = TripRecord(
                id: TripID(UUID().uuidString.lowercased()),
                name: trimmed.isEmpty ? "New trip" : trimmed,
                stageIDs: [routeID],
                addedAt: plannedRecords[routeID]?.addedAt ?? now()
            )
            library.saveTrip(trip)
            reloadTrips()
        }
    }

    /// Remove a route from whatever trip holds it (the route menu's "Remove from
    /// trip") — the route returns to the top level, its record untouched;
    /// emptying the trip dissolves it. A no-op on a loose route.
    public func removeRouteFromTrip(_ routeID: RouteID) {
        guard let tripID = tripContaining(routeID) else { return }
        _ = removeStage(routeID, from: tripID)
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
        // Also prunes the id from any trip that held it, dissolving a trip left
        // with no stages (the store's contract) — re-read so the list reflects it.
        library.deletePlannedRoute(id)
        reloadTrips()
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
            rebuildPlannedItems()
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
        rebuildPlannedItems()
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

    /// The device object id this planned route is stored under **on the
    /// connected device, in its current era** — threaded into a re-upload so
    /// it replaces that object instead of duplicating. Gated by the validity
    /// predicate (#769): a link minted on another device or in a previous
    /// era answers `nil`, so replace-by-id can never overwrite an object the
    /// link doesn't actually point at (the v1 wrong-route-overwrite bug); the
    /// upload then creates a fresh copy — the safe direction.
    public func plannedDeviceObjectID(for id: RouteID) -> DeviceObjectID? {
        guard let link = plannedRecords[id]?.deviceLink, let scope = connectedScope,
            link.matches(scope)
        else { return nil }
        return link.objectID
    }

    /// The CRC the connected device is **proven** to hold for this route (#770)
    /// — threaded into the detail so its button reads "up to date" only on the
    /// same proof the list badge uses (a scoped link + a matching non-zero
    /// catalog CRC), never on link presence alone. `nil` when unproven → the
    /// detail offers Upload, not a disabled "up to date".
    public func plannedProvenCommittedCRC(for id: RouteID) -> UInt32? {
        guard let record = plannedRecords[id] else { return nil }
        return provenCommittedCRC(for: record)
    }

    // MARK: Route retention — S7 UI seams (epic #638)

    /// The app-local default retention a **new** upload seeds (Settings picks it).
    /// The upload sheet's Auto-delete row and its post-commit push both start here.
    public var defaultRetention: Retention { retentionDefaults.loadDefaultRetention() }

    /// The **desired** app-side retention for this planned route (`nil` = not set),
    /// used to seed the upload sheet and the detail control. Distinct from the
    /// device's reported level.
    public func plannedRetention(for id: RouteID) -> Retention? {
        plannedRecords[id]?.retention
    }

    /// The library-card countdown footnote (C1) for a route on the device — the
    /// device's `expires_at` phrased "Expires in 2 days" / "Expires today", but
    /// **only** within ``OBCFormat/expiryBadgeDayWindow`` days and while the route
    /// is actually on the device (the badge disappears with the on-device state
    /// once a device-side delete reconciles to `notOnDevice`). `nil` otherwise.
    public func expiryBadge(for id: RouteID) -> String? {
        guard onDeviceState(id) != .notOnDevice,
            let expiresAt = plannedRecords[id]?.deviceExpiresAt
        else { return nil }
        return OBCFormat.routeExpiryBadge(expiresAt, relativeTo: now())
    }

    /// The device's actual retention level for this route (`nil` = unknown /
    /// pre-expiry firmware) — the route detail falls back to it for the row value
    /// when no desired level is set, so the row doesn't claim "Never" over a live
    /// expiry. Display-only; the push still gates on the *desired* level.
    public func plannedDeviceRetention(for id: RouteID) -> Retention? {
        plannedRecords[id]?.deviceRetention
    }

    /// The device's expiry truth for this route (`nil` = never / not started /
    /// pre-expiry firmware) — the route detail formats it into its "Expires …"
    /// line. Display-only; it goes stale gracefully (extend-on-use moves it).
    public func plannedDeviceExpiresAt(for id: RouteID) -> Date? {
        plannedRecords[id]?.deviceExpiresAt
    }

    /// Edit a route's desired retention from its detail (S7). Stores the choice on
    /// the record (persisted), then — connected, capable, and holding a valid link
    /// for a level that diverges from the device's — pushes it now; disconnected,
    /// it just stores and the next connect's reconcile pushes it (the desired level
    /// still diverges). No "pending" chrome: the reconcile model makes it
    /// eventually-true. A retro change to the default never lands here — only an
    /// explicit per-route edit does.
    public func setRouteRetention(_ id: RouteID, _ retention: Retention) {
        guard var record = plannedRecords[id], record.retention != retention else { return }
        record.retention = retention
        // Push now when the device holds this route (valid scoped link), is
        // capable, and the desired level diverges from the device's — optimistic,
        // like the reconcile push; a failed send self-heals at the next reconcile.
        if supportsRetention, let scope = connectedScope, let link = record.deviceLink,
            link.matches(scope), record.deviceRetention != retention {
            pushRetention(retention, to: link.objectID)
            record.deviceRetention = retention  // optimistic; a later list confirms
        }
        plannedRecords[id] = record
        library.savePlannedRoute(record)
    }

    /// H3 write-through from Settings (B8) — the top bar shows the new device
    /// name at once; Settings owns the config write and the bond record.
    public func deviceRenamed(to name: String) {
        deviceName = name
    }

    /// A B5 upload committed — record the `{serial, epoch, id}` link it landed
    /// under (#769) so the C1 badge lights and a later re-upload replaces that
    /// object *on that device in that era*. Idempotent; a new link (re-upload
    /// after a device-side change) overwrites the old. The scope comes from
    /// the connection's settled identity; in the vanishing window where an
    /// upload commits before/without it, no link is recorded — the safe
    /// direction (no badge, the next push or V6's adoption re-links) — because
    /// a scope-less link is exactly the v1 aliasing this change retires.
    public func markRouteUploaded(
        _ id: RouteID, objectID: DeviceObjectID, crc32: UInt32, retention: Retention? = nil
    ) {
        markRouteUploaded(id, objectID: objectID, crc32: crc32, retention: retention, adopt: true)
    }

    /// The commit itself, with the adoption rule made optional: a **single**
    /// route upload adopts (pushes its trip object if the trip is on device); a
    /// stage committed **inside** a whole-trip upload does not — that queue pushes
    /// the trip object once, at the end, so a per-stage adoption would be a
    /// redundant (and racing) trip push.
    func markRouteUploaded(
        _ id: RouteID, objectID: DeviceObjectID, crc32: UInt32, retention: Retention?, adopt: Bool
    ) {
        guard var record = plannedRecords[id] else { return }
        if let scope = connectedScope {
            record.deviceLink = DeviceRouteLink(
                serial: scope.serial, epoch: scope.epoch, objectID: objectID)
            record.uploadedCRC32 = crc32
            // The transfer verified this whole-object CRC for this object, so
            // record it as device truth (#770): the badge proves immediately,
            // before the next `listRoutes()` catches up. A later catalog read
            // overwrites this with what the device actually reports.
            deviceRouteCRCs[objectID] = crc32
            // Retention opt-in on upload (epic #638): an upload is an explicit
            // "put this on the device now". The upload sheet's Auto-delete row
            // (S7) passes the rider's chosen level; without one, a route with no
            // desired level yet takes the app-local default, and an existing
            // choice is kept. Push it so the fresh route gets its expiry without a
            // second upload — the device stamps `last_used = now` at commit, so
            // `expires_at = now + retention`. Capability-gated (skipped, but the
            // desired level is still recorded for a later reconcile push against
            // newer firmware).
            let chosen = retention ?? record.retention ?? retentionDefaults.loadDefaultRetention()
            record.retention = chosen
            if supportsRetention {
                pushRetention(chosen, to: objectID)
                record.deviceRetention = chosen  // optimistic; a later list confirms
            }
        } else {
            record.deviceLink = nil
            record.uploadedCRC32 = nil
        }
        plannedRecords[id] = record
        library.savePlannedRoute(record)
        refreshOnDeviceStates()
        // Adoption rule (TR8, locked in the epic): a single route that belongs to
        // an app trip already on the device files into the folder — push the
        // updated trip object. Otherwise the route lands standalone. Runs after
        // the route's own commit so the trip object carries the fresh stage id.
        if adopt { maybeAdoptRouteIntoDeviceTrip(id) }
    }

    /// A whole-trip upload committed the trip object under `objectID` (TR8) —
    /// record the `{serial, epoch, id}` link + fingerprint so the trip badge
    /// lights and a later push replaces that object in place. Idempotent; the
    /// route-upload rule in reverse (no scope, or no committed id → no link, the
    /// safe direction: the next push or reconcile re-links).
    public func markTripUploaded(_ id: TripID, objectID: DeviceObjectID?, crc32: UInt32) {
        guard var trip = trip(id) else { return }
        if let scope = connectedScope, let objectID {
            trip.deviceLink = DeviceRouteLink(serial: scope.serial, epoch: scope.epoch, objectID: objectID)
            trip.uploadedCRC32 = crc32
            // The transfer verified this whole-object CRC — record it as device
            // truth so the badge proves before the next `listTrips()` catches up.
            deviceTripCRCs[objectID] = crc32
        } else {
            trip.deviceLink = nil
            trip.uploadedCRC32 = nil
        }
        library.saveTrip(trip)
        reloadTrips()
    }

    /// The adoption rule's trip-object push (TR8): iff the route's trip already
    /// exists on the device (a valid scoped link confirmed by the last reconcile),
    /// push the updated trip object so the newly-committed route files into the
    /// folder. Best-effort — a failed push leaves the trip page reading outdated,
    /// which the Upload-trip button (or the next reconnect reconcile) heals.
    private func maybeAdoptRouteIntoDeviceTrip(_ routeID: RouteID) {
        guard let scope = connectedScope,
            let tripID = tripContaining(routeID),
            let trip = trip(tripID),
            let link = trip.deviceLink, link.matches(scope),
            // Confirmed on the device: a non-zero CRC for the trip object (poked
            // by the last commit or a `listTrips()` reconcile) — the same proof
            // the badge uses, so an in-session upload counts without re-reading.
            (deviceTripCRCs[link.objectID] ?? 0) != 0
        else { return }
        pushTripObject(tripID, replacing: link.objectID)
    }

    /// Encode + upload one trip object (replace-by-id), committing the link on
    /// success — the metadata-only push shared by the adoption rule and a
    /// fully-current "Upload trip". Fire-and-forget; reconcile is the backstop.
    private func pushTripObject(_ tripID: TripID, replacing objectID: DeviceObjectID?) {
        guard let trip = trip(tripID) else { return }
        let deviceStageIDs = currentTripDeviceStageIDs(for: trip)
        let payload = TripObjectCodec.encode(name: trip.name, deviceStageIDs: deviceStageIDs)
        let crc = CRC32.checksum(payload)
        let blob = TripBlob(
            name: trip.name, deviceStageIDs: deviceStageIDs, payload: payload, targetObjectID: objectID)
        Task { [weak self, transport] in
            let handle = transport.uploadTrip(blob)
            guard await handle.outcome == .completed else { return }
            let assigned = await handle.assignedObjectID
            guard let self else { return }
            self.markTripUploaded(tripID, objectID: assigned ?? objectID, crc32: crc)
        }
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
