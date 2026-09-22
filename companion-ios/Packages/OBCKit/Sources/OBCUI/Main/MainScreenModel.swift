import Foundation
import Observation
import OBCDomain
import OBCTransport

@MainActor @Observable
public final class MainScreenModel {
    public enum Tab: Int, Sendable {
        case planned = 0
        case tracked = 1
    }

    public enum LoadState: Equatable, Sendable {
        case loading
        case loaded
        case failed
    }

    /// A connected device whose reported protocol version differs from `OBCProtocol.version`.
    /// The app must not decode objects from such a device, so sync stays disabled.
    public struct ProtocolMismatch: Equatable, Sendable {
        public var expected: UInt16
        public var found: UInt16
    }

    // MARK: Observable state

    public private(set) var deviceName = "Your OBC"
    public private(set) var connection: ConnectionState = .connecting
    public private(set) var battery: Int?
    public private(set) var loadState: LoadState = .loading
    public private(set) var routes: [RouteSummary] = []
    /// Each planned route's proven device-copy state, behind the list badge. Observable, so
    /// the badge moves the instant an upload commits or a re-import changes the content.
    public private(set) var onDevice: [RouteID: OnDeviceState] = [:]
    /// Every saved trip, newest first.
    public private(set) var trips: [TripRecord] = []
    /// The Planned tab rows: trip cards and loose route cards, newest first. A route filed in
    /// a trip shows only inside that trip; `routes` keeps every planned summary.
    public private(set) var plannedItems: [PlannedItem] = []
    public private(set) var rides: [RideSummary] = []
    /// Trashed rides, most recently trashed first.
    public private(set) var trashedRides: [RideSummary] = []
    public var tab: Tab = .planned
    public var searchText = ""
    public private(set) var protocolMismatch: ProtocolMismatch?
    /// The connected device's serial and StoreId. Nil keeps reconciliation disabled while
    /// local browsing continues.
    public private(set) var connectedScope: LibraryScope?
    /// True once the identity read settles, with an answer or with a failed read. The other
    /// half of the `canSync` gate, so an id-keyed write never runs ahead of the verdict.
    @ObservationIgnored private var identityChecked = false

    public let sync: RideSyncCoordinator

    // MARK: Derived

    /// A dropped link degrades to a banner over browsable content, never an error. While an
    /// interrupted sync waits for Resume, that banner tells the link story instead.
    public var showsDisconnectedBanner: Bool {
        (connection == .outOfRange || connection == .disconnected) && sync.syncInterruption == nil
    }

    public var filteredRoutes: [RouteSummary] {
        filtered(routes, by: \.name)
    }

    public var filteredPlannedItems: [PlannedItem] {
        filtered(plannedItems, by: \.name)
    }

    public var filteredRides: [RideSummary] {
        filtered(rides, by: \.name)
    }

    // MARK: Wiring

    private let transport: any DeviceLink & DeviceBattery & DeviceObjects & DeviceClock
    private let library: any LibraryStore
    private let lastBikeType: LastBikeTypeStore
    /// Runs once per established connection. Nil in tests and previews skips it.
    private let nameReconciler: DeviceNameReconciler?
    /// Device rides deleted on the phone. The merge hides them so a sync cannot restore them.
    @ObservationIgnored private var deletedRideIDs: Set<RideID> = []
    /// Rides in Recently Deleted, with the time each was trashed.
    @ObservationIgnored private var trashedRideIDs: [RideID: Date] = [:]
    @ObservationIgnored private var plannedRecords: [RouteID: PlannedRouteRecord] = [:]
    /// Ride summaries only. Tracklogs stay on disk and load one ride at a time through
    /// `ride(_:)`.
    @ObservationIgnored private var rideSummaries: [RideID: RideSummary] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    /// Set when a reload is requested while `loadTask` is already reading. Bursts coalesce
    /// here instead of cancelling a live transfer.
    @ObservationIgnored private var reloadRequested = false
    /// The last route catalog a reload read. On launch the catalog usually arrives before the
    /// identity verdict, so the reconcile re-runs once the scope settles.
    @ObservationIgnored private var lastRouteCatalog: [RouteCatalogEntry]?
    /// Per-object content CRCs from the device route catalog. A link is a checkmark only when
    /// this holds a non-zero CRC for its object equal to the record's committed fingerprint.
    /// Zero, or an absent key, proves nothing.
    @ObservationIgnored private var deviceRouteCRCs: [DeviceObjectID: UInt32] = [:]
    /// The last trip catalog a reload read, the trip sibling of `lastRouteCatalog`.
    @ObservationIgnored private var lastTripCatalog: [TripCatalogEntry]?
    /// Per-trip content CRCs from the device trip catalog, the trip half of `deviceRouteCRCs`.
    @ObservationIgnored private var deviceTripCRCs: [DeviceObjectID: UInt32] = [:]
    /// The in-flight transfer ledger. Nil in tests and previews.
    @ObservationIgnored private let transferActivity: TransferActivity?
    /// The in-flight identity read for the current connection. A reconnect replaces it and
    /// never cancels it: a superseded read settles the same session-stable verdict.
    @ObservationIgnored private var identityTask: Task<Void, Never>?

    public static let trashRetentionDays = 30

    private let now: () -> Date

    public init(
        transport: any DeviceLink & DeviceBattery & DeviceObjects & DeviceClock,
        library: any LibraryStore = InMemoryLibraryStore(),
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore(),
        syncTiming: RideSyncCoordinator.Timing = RideSyncCoordinator.Timing(),
        nameReconciler: DeviceNameReconciler? = nil,
        transferActivity: TransferActivity? = nil,
        now: @escaping () -> Date = Date.init
    ) {
        self.transport = transport
        self.library = library
        self.lastBikeType = lastBikeType
        self.nameReconciler = nameReconciler
        self.transferActivity = transferActivity
        self.now = now
        self.sync = RideSyncCoordinator(
            transport: transport, library: library, timing: syncTiming,
            activity: transferActivity
        )
        // Weak captures: the coordinator's closures must never pin the model.
        sync.canSync = { [weak self] in
            guard let self else { return false }
            return identityChecked && protocolMismatch == nil && connectedScope != nil
        }
        sync.identitySettled = { [weak self] in await self?.identityTask?.value }
        sync.onRideCatalogRead = { [weak self] in self?.loadState = .loaded }
        // The coordinator already persisted the ride; mirror the summary so it shows at once.
        sync.onRideLanded = { [weak self] ride in
            guard let self else { return }
            rideSummaries[ride.id] = ride.summary
            rides = trackedList()
        }
    }

    // MARK: Lifecycle

    /// Subscribe the live streams and load the library. Call once.
    public func start() {
        guard !started else { return }
        started = true

        // Library first: the lists are browsable before a device read lands, or without one.
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

        // The streams never finish, so a strong capture would pin the model for the session.
        streamTasks.append(Task { [weak self, transport] in
            var previous: ConnectionState?
            for await state in transport.state {
                guard let self else { return }
                connection = state
                // A regained link, never the stream's replayed first value: re-read the lists
                // and run the desired-name reconcile for a config write that never landed.
                if state == .connected, let was = previous, was != .connected {
                    reload()
                    // Re-read identity because firmware or the mounted store can change.
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
            for await change in transport.catalogChanges {
                guard let self else { return }
                // The device store moved under an open app. Re-read so the "on device" badge
                // is true again without a reconnect. Rides move only through Sync, so only
                // route and trip movements trigger a reload.
                if change.kind == .route || change.kind == .trip { reload() }
            }
        })
        // Protocol v4 has no store-change notification, so audit the small route and trip
        // catalogs on a timer. Without it an external change leaves a stale "on device" badge.
        streamTasks.append(Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                guard !Task.isCancelled, let self else { return }
                if connection == .connected { reload() }
            }
        })
        reload()
        // Identity after the first library read, so a fault armed for the first read hits the
        // lists and not this fetch. The same read carries the protocol-version check.
        let firstLoad = loadTask
        identityTask = Task { [weak self] in
            await firstLoad?.value
            await self?.runIdentityCheck()
        }
        // The reconnect edge never fires for the stream's replayed first value, so the launch
        // connection reconciles here. Launched disconnected, the pass skips silently.
        if let nameReconciler {
            streamTasks.append(Task {
                await firstLoad?.value
                await nameReconciler.reconcile()
            })
        }
    }

    deinit {
        // The sync coordinator cancels its own tasks.
        streamTasks.forEach { $0.cancel() }
        loadTask?.cancel()
        identityTask?.cancel()
    }

    /// Re-read both lists, and the read-error retry. Cached content stays up while the fresh
    /// read runs; only an empty library shows skeletons.
    public func reload() {
        // Never decode an incompatible device's objects. The library-first content stays up
        // and the banner explains.
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

    /// Drain coalesced catalog requests serially. Main-actor isolation stops the dirty bit and
    /// the `loadTask` retirement from racing a store-change callback.
    private func runReloadLoop() async {
        while reloadRequested, !Task.isCancelled {
            reloadRequested = false
            do {
                // Only the route catalog: Tracked is library-first, so its rows come from the
                // local library and device rides are pulled by Sync alone.
                let deviceRoutes = try await transport.listRoutes()
                guard !Task.isCancelled else { break }
                lastRouteCatalog = deviceRoutes
                reconcileOnDevice(with: deviceRoutes)
                // Fail closed: a failed trip read skips the reconcile, because treating it as
                // "zero trips" drops every trip link, and the next upload then mints a
                // duplicate device trip instead of replacing in place.
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

    /// Read identity before scope-filtered reconciliation.
    /// A missing scope or failed read leaves device writes disabled until the
    /// next successful connection. Local library browsing remains available.
    private func runIdentityCheck() async {
        // Unknown until proven, every connection: the device may have been
        // reinitialized (new StoreId) or swapped since the last read.
        connectedScope = nil
        await stampDeviceClock()
        if let info = try? await transport.deviceInfo() {
            deviceName = info.name
            if case let .protocolMismatch(expected, found)? =
                OBCProtocol.versionMismatch(reportedBy: info.protocolVersion) {
                protocolMismatch = ProtocolMismatch(expected: expected, found: found)
            } else {
                protocolMismatch = nil
                // `libraryScope` is nil on a missing StoreId or empty serial. Never defaulted.
                connectedScope = info.libraryScope
            }
        }
        identityChecked = true
        if connectedScope != nil {
            // The reload may have run before the scope was known; true the links up now.
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

    private func stampDeviceClock() async {
        guard let outcome = try? await transport.setClock(WallClockSample()) else { return }
    }
    /// True up every record's `deviceLink` against the device catalog, then adopt by content.
    /// Absence, or a catalog CRC that disagrees with what we committed, drops the link; an
    /// unlinked entry whose CRC matches a record's current encoding re-links it.
    ///
    /// Fail closed on scope: an unknown identity writes no link at all, and a known one clears
    /// only links that match the connected scope.
    private func reconcileOnDevice(with deviceRoutes: [RouteCatalogEntry]) {
        // Refreshed wholesale from the device, replacing any optimistic post-upload poke.
        deviceRouteCRCs = Dictionary(
            deviceRoutes.map { ($0.id, $0.crc32) }, uniquingKeysWith: { first, _ in first })
        guard let scope = connectedScope else {
            refreshOnDeviceStates()
            return
        }
        let listed = Set(deviceRoutes.map(\.id))
        // 1) Drop links the catalog disproves: an absent object, or a present one whose
        //    non-zero CRC differs from our fingerprint. A `crc32 = 0` entry proves nothing,
        //    so that link is kept.
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
        // 2) Adopt by content: heal identical unlinked copies without a re-upload.
        adoptByContent(scope: scope, catalog: deviceRoutes)
        refreshOnDeviceStates()
    }

    /// Re-link an unlinked record to a catalog entry that holds the same content, so the next
    /// upload replaces that object instead of minting a duplicate. Heals an app reinstall or a
    /// device switch-back silently.
    ///
    /// The ObjectId is identity; the payload CRC is only a content fingerprint. A rename moves
    /// the bytes without changing the route, so the catalog entry's name is spliced back in to
    /// reconstruct the stored bytes. The adopted record pins the entry's CRC, so the badge
    /// reads "out of date" and the next send is a same-id replace that carries the new name.
    ///
    /// Ambiguity resolves first-come, each side claimed at most once.
    private func adoptByContent(scope: LibraryScope, catalog: [RouteCatalogEntry]) {
        // Object ids already spoken for by a valid link — never adopt over them.
        var claimed = Set(plannedRecords.values.compactMap { record -> DeviceObjectID? in
            guard let link = record.deviceLink, link.matches(scope) else { return nil }
            return link.objectID
        })
        let adoptable = catalog.filter { $0.crc32 != 0 && !claimed.contains($0.id) }
        guard !adoptable.isEmpty else { return }
        // Deterministic order, so an adoption is reproducible run to run.
        let candidates = plannedRecords.values
            .filter { record in
                guard let link = record.deviceLink else { return true }
                return !link.matches(scope)
            }
            .sorted { $0.id.rawValue < $1.id.rawValue }
        for record in candidates {
            let payload = RouteObjectCodec.encode(
                points: record.route.points, waypoints: record.route.waypoints, name: record.summary.name,
                bikeType: record.bikeType)
            let currentCRC = CRC32.checksum(payload)
            guard let entry = adoptable.first(where: { entry in
                guard !claimed.contains(entry.id) else { return false }
                if entry.crc32 == currentCRC { return true }
                // The rename case: the device's copy is still under the catalog's name.
                guard entry.name != record.summary.name else { return false }
                return entry.crc32 == CRC32.checksum(RouteObjectCodec.renamed(payload, to: entry.name))
            }) else { continue }
            var adopted = record
            adopted.deviceLink = DeviceRouteLink(scope: scope, objectID: entry.id)
            adopted.uploadedCRC32 = entry.crc32
            plannedRecords[record.id] = adopted
            library.savePlannedRoute(adopted)
            claimed.insert(entry.id)
        }
    }

    /// The CRC the connected device is proven to hold for this record, or nil when unproven:
    /// a scoped link, a non-zero catalog CRC, and equality with the committed fingerprint.
    private func provenCommittedCRC(for record: PlannedRouteRecord) -> UInt32? {
        guard let scope = connectedScope, let link = record.deviceLink,
            link.matches(scope), let uploaded = record.uploadedCRC32,
            let catalogCRC = deviceRouteCRCs[link.objectID], catalogCRC != 0,
            catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    private func refreshOnDeviceStates() {
        onDevice = plannedRecords.mapValues { record in
            OnDeviceState.determine(
                provenCommittedCRC: provenCommittedCRC(for: record),
                currentCRC: { RouteObjectCodec.payloadCRC(for: record) }
            )
        }
    }

    private func plannedList() -> [RouteSummary] {
        plannedRecords.values.sorted { $0.addedAt > $1.addedAt }.map(\.summary)
    }

    // MARK: Trips

    /// Re-read trips and rebuild the interleaved Planned list. Every trip or route edit ends
    /// on this call.
    func reloadTrips() {
        trips = library.trips()
        rebuildPlannedItems()
    }

    private func rebuildPlannedItems() {
        plannedItems = PlannedItem.partition(records: Array(plannedRecords.values), trips: trips)
    }

    public func trip(_ id: TripID) -> TripRecord? { trips.first { $0.id == id } }

    public var tripPickerItems: [TripPickerItem] {
        trips.map { TripPickerItem(id: $0.id, name: $0.name, stageCount: $0.stageIDs.count) }
    }

    public func tripContaining(_ routeID: RouteID) -> TripID? {
        trips.first { $0.stageIDs.contains(routeID) }?.id
    }

    /// A trip's member routes as list summaries, in ride order.
    public func tripStages(_ id: TripID) -> [RouteSummary] {
        guard let trip = trip(id) else { return [] }
        return trip.stageIDs.compactMap { plannedRecords[$0]?.summary }
    }

    public func tripStats(_ id: TripID) -> TripStats {
        TripStats.summing(tripStages(id))
    }

    /// The trip badge: up to date only when the trip object itself is proven current and every
    /// stage is up to date. A trip the phone never pushed reads `.notOnDevice`.
    public func tripOnDeviceState(_ id: TripID) -> OnDeviceState {
        guard let trip = trip(id), !trip.stageIDs.isEmpty else { return .notOnDevice }
        let tripSelf = OnDeviceState.determine(
            provenCommittedCRC: provenTripCommittedCRC(for: trip),
            currentCRC: { currentTripPayloadCRC(for: trip) }
        )
        let stageStates = trip.stageIDs.map { onDeviceState($0) }
        return Self.composeTripState(tripSelf: tripSelf, stageStates: stageStates)
    }

    /// Each stage's committed device object id, in ride order, dropping any stage the device
    /// does not hold. The one definition the encode, the fingerprint and the plan all read.
    private func currentTripDeviceStageIDs(for trip: TripRecord) -> [DeviceObjectID] {
        trip.stageIDs.compactMap { plannedDeviceObjectID(for: $0) }
    }

    /// The trip object an upload would send now: one whole day per resolved stage. The one
    /// definition the encode and the fingerprint read.
    private func currentTripObject(for trip: TripRecord, stageIDs: [DeviceObjectID]) -> TripObjectCodec.Trip {
        TripObjectCodec.Trip(
            key: trip.key, name: trip.name, startDate: 0, days: stageIDs.map(TripObjectCodec.Day.whole))
    }

    /// The CRC of the trip object an upload would send now.
    private func currentTripPayloadCRC(for trip: TripRecord) -> UInt32 {
        TripObjectCodec.payloadCRC(currentTripObject(for: trip, stageIDs: currentTripDeviceStageIDs(for: trip)))
    }

    /// The trip twin of `provenCommittedCRC(for:)`.
    private func provenTripCommittedCRC(for trip: TripRecord) -> UInt32? {
        guard let scope = connectedScope, let link = trip.deviceLink, link.matches(scope),
            let uploaded = trip.uploadedCRC32,
            let catalogCRC = deviceTripCRCs[link.objectID], catalogCRC != 0,
            catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    /// True up every trip's `deviceLink` against the device trip catalog: drop pass, then adopt
    /// pass. A local edit (rename, reorder) never drops a link; it leaves the committed CRC
    /// intact and reads as outdated. Adoption also heals a lost commit ack, where the trip
    /// object landed but the ack did not, which otherwise mints a same-name twin folder.
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

    /// `adoptByContent`'s trip twin, with the same tie-breaks. The fingerprint is the trip's
    /// current encoding, so both call sites run this after the route reconcile has trued the
    /// stage links up.
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
            let stageIDs = currentTripDeviceStageIDs(for: trip)
            let object = currentTripObject(for: trip, stageIDs: stageIDs)
            let currentCRC = TripObjectCodec.payloadCRC(object)
            guard let entry = adoptable.first(where: { entry in
                guard !claimed.contains(entry.id) else { return false }
                if entry.crc32 == currentCRC { return true }
                // The rename case: the device's copy is still under the catalog's name.
                guard entry.name != trip.name else { return false }
                var renamed = object
                renamed.name = entry.name
                return entry.crc32 == TripObjectCodec.payloadCRC(renamed)
            }) else { continue }
            trip.deviceLink = DeviceRouteLink(scope: scope, objectID: entry.id)
            // The entry's CRC is what the device holds, so the trip reads as out of date and
            // the next send replaces by id.
            trip.uploadedCRC32 = entry.crc32
            library.saveTrip(trip)
            claimed.insert(entry.id)
        }
    }

    // MARK: Whole-trip upload

    /// Partition the stages into skip, replace and fresh, and do the precheck math. Nil when
    /// the trip has dissolved.
    public func planTripUpload(_ id: TripID) -> TripUploadPlan? {
        guard let trip = trip(id) else { return nil }
        let stageInputs = trip.stageIDs.map { routeID in
            TripUploadPlanner.StageInput(
                routeID: routeID,
                isUpToDate: onDeviceState(routeID) == .upToDate,
                committedObjectID: plannedDeviceObjectID(for: routeID)
            )
        }
        // A valid scoped link is the replace target. Do not re-check the cached catalog: a
        // stale cache demotes a valid link to a fresh upload, and a fresh upload of an
        // already-stored trip mints a silent duplicate. A replace of a vanished trip fails
        // loudly instead, which is the safe side.
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

    /// Re-read both catalogs and reconcile before planning: the retry-after-failure path. A
    /// stage, or the trip object, that committed but whose ack was lost would otherwise re-plan
    /// as fresh and mint a device twin. Routes before trips; the adoption rule needs that order.
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

    /// Turn the plan into a queue: a step per stage in ride order, then the trip object last.
    /// Nothing is sent when every stage and the trip object are already current. Each step
    /// commits its own link the instant it lands. Nil when the trip dissolved.
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
                        self?.markRouteUploaded(
                            routeID, objectID: objectID, crc32: crc, adopt: false)
                    }
                ))
            }
        }
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
            precheck: plan.precheck, steps: steps,
            timing: timing, activity: transferActivity
        )
    }

    private func makeStageBlob(_ routeID: RouteID, target: DeviceObjectID?) -> RouteBlob? {
        guard let record = plannedRecords[routeID] else { return nil }
        let payload = RouteObjectCodec.encode(
            points: record.route.points, waypoints: record.route.waypoints, name: record.summary.name,
            bikeType: record.bikeType)
        guard !payload.isEmpty else { return nil }
        return RouteBlob(
            summary: record.summary, waypoints: record.route.waypoints,
            payload: payload, targetObjectID: target)
    }

    /// Built at execution time, after the stages committed, so it carries their fresh device
    /// ids. Nil when no stage resolves to a device copy: there is nothing to reference.
    private func makeTripBlob(_ tripID: TripID, target: DeviceObjectID?) -> TripBlob? {
        guard let trip = trip(tripID) else { return nil }
        let deviceStageIDs = currentTripDeviceStageIDs(for: trip)
        guard !deviceStageIDs.isEmpty else { return nil }
        let payload = TripObjectCodec.encode(currentTripObject(for: trip, stageIDs: deviceStageIDs))
        return TripBlob(
            name: trip.name, deviceStageIDs: deviceStageIDs, payload: payload, targetObjectID: target)
    }

    /// Pure and static, so the rule is testable without a device.
    static func composeTripState(
        tripSelf: OnDeviceState, stageStates: [OnDeviceState]
    ) -> OnDeviceState {
        guard !stageStates.isEmpty, tripSelf != .notOnDevice else { return .notOnDevice }
        if tripSelf == .upToDate, stageStates.allSatisfy({ $0 == .upToDate }) { return .upToDate }
        return .outdated
    }

    /// Rename a trip. Phone-local; the new name rides the next trip upload.
    public func renameTrip(_ id: TripID, to name: String) {
        guard var trip = trip(id) else { return }
        trip.name = name
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Reorder a trip's stages. Ride order is the trip's source of truth, and a reorder
    /// out-dates the device copy.
    public func reorderTripStages(_ id: TripID, from source: IndexSet, to destination: Int) {
        guard var trip = trip(id) else { return }
        trip.stageIDs.move(fromOffsets: source, toOffset: destination)
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Remove one stage; the route returns to the top level. Removing the last stage dissolves
    /// the trip and returns `true`, so the caller can pop the page.
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

    /// Drop the trip metadata. Every member route stays in the library and returns to the top
    /// level.
    public func ungroupTrip(_ id: TripID) {
        library.deleteTrip(id)
        reloadTrips()
    }

    /// Delete each member route's device copy and the trip object while connected, then the
    /// phone library. Offline, only the phone copies go and the device copies surface as
    /// orphans at the next reconcile.
    public func deleteTripAndRoutes(_ id: TripID) {
        guard let trip = trip(id) else { return }
        let stages = trip.stageIDs
        // The protocol trip delete does not cascade, so compose it here. Best-effort: a failed
        // command leaves an orphan the reconcile heals.
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

    // MARK: Create & file

    /// Group the selected routes into a new trip. Stages take Planned-list order, not selection
    /// order, and reordering stays the trip page's job. The new trip takes the slot of its
    /// newest member. Ids with no live record are dropped; an empty result creates nothing.
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
            addedAt: ordered.map(\.addedAt).max() ?? now()
        )
        library.saveTrip(trip)  // ≤ 1-trip invariant enforced in the store
        reloadTrips()
        return trip.id
    }

    /// File a route per a picker selection. `.existing` appends it as the trip's last stage;
    /// the store strips it from any other trip, so a move is an implicit remove. `.new` starts
    /// a trip in that route's list slot. Phone-local: library writes only.
    public func fileRoute(_ routeID: RouteID, into selection: TripSelection) {
        switch selection {
        case .none:
            break
        case .existing(let tripID):
            guard var trip = trip(tripID), !trip.stageIDs.contains(routeID) else { return }
            trip.stageIDs.append(routeID)
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

    public func removeRouteFromTrip(_ routeID: RouteID) {
        guard let tripID = tripContaining(routeID) else { return }
        _ = removeStage(routeID, from: tripID)
    }

    // MARK: Delete

    /// Remove a planned route from the phone. Never from the device: a copy already there
    /// stays, mirroring the ride rule in reverse.
    public func deleteRoute(_ id: RouteID) {
        routes.removeAll { $0.id == id }
        plannedRecords[id] = nil
        onDevice[id] = nil
        // The store also prunes the id from any trip, and dissolves a trip left with no stages.
        library.deletePlannedRoute(id)
        reloadTrips()
    }

    /// Move a tracked ride to Recently Deleted. The device copy stays, and so do the stored
    /// files, which is what makes Recover instant. The id stays marked synced, so the next sync
    /// does not download it again.
    public func deleteRide(_ id: RideID) {
        rides.removeAll { $0.id == id }
        let date = now()
        trashedRideIDs[id] = date
        library.markRideTrashed(id, at: date)
        library.markRideSynced(id)
        trashedRides = trashedList()
    }

    public func recoverRide(_ id: RideID) {
        trashedRideIDs[id] = nil
        library.unmarkRideTrashed(id)
        rides = trackedList()
        trashedRides = trashedList()
    }

    /// Delete a trashed ride's files. The durable tombstone takes over: the id stays marked
    /// synced, so a sync does not re-download it, and marked deleted, so the merge does not
    /// re-list the device's copy.
    public func deleteRideForever(_ id: RideID) {
        trashedRideIDs[id] = nil
        library.unmarkRideTrashed(id)
        rideSummaries[id] = nil
        library.deleteRide(id)
        deletedRideIDs.insert(id)
        library.markRideDeleted(id)
        trashedRides = trashedList()
    }

    private func purgeExpiredTrash() {
        let cutoff = now().addingTimeInterval(-TimeInterval(Self.trashRetentionDays) * 86_400)
        for (id, date) in trashedRideIDs where date < cutoff {
            deleteRideForever(id)
        }
    }

    // MARK: Rename and import landing

    /// Rename a planned route. Phone-local; the name reaches the device on the next upload, so
    /// a rename out-dates the device copy until then.
    public func renameRoute(_ id: RouteID, to name: String) {
        guard let index = routes.firstIndex(where: { $0.id == id }) else { return }
        routes[index].name = name
        if var record = plannedRecords[id] {
            record.summary.name = name
            plannedRecords[id] = record
            library.savePlannedRoute(record)
            refreshOnDeviceStates()
            rebuildPlannedItems()
        }
    }

    /// Set a planned route's bike type, and make it the type the next import starts with. The type
    /// rides in the payload, so the change out-dates the device copy until the next upload.
    public func setBikeType(_ id: RouteID, to type: BikeType) {
        lastBikeType.value = type
        guard var record = plannedRecords[id] else { return }
        record.bikeType = type
        record.summary.estimatedDuration = type.estimatedDuration(
            distanceMeters: record.summary.distanceMeters, ascentMeters: record.summary.elevationGainMeters)
        plannedRecords[id] = record
        library.savePlannedRoute(record)
        if let index = routes.firstIndex(where: { $0.id == id }) { routes[index] = record.summary }
        refreshOnDeviceStates()
        rebuildPlannedItems()
    }

    /// Rename a tracked ride, with the same phone-local rule. A summary-only write: the
    /// tracklog on disk is untouched.
    public func renameRide(_ id: RideID, to name: String) {
        guard let index = rides.firstIndex(where: { $0.id == id }) else { return }
        rides[index].name = name
        if var summary = rideSummaries[id] {
            summary.name = name
            rideSummaries[id] = summary
            library.saveRideSummary(summary)
        }
    }

    /// Change a tracked ride's bike type. Phone-local, like a rename: the device copy keeps the
    /// type the ride started with.
    public func setRideBikeType(_ id: RideID, to type: BikeType) {
        guard let index = rides.firstIndex(where: { $0.id == id }) else { return }
        rides[index].bikeType = type
        if var summary = rideSummaries[id] {
            summary.bikeType = type
            rideSummaries[id] = summary
            library.saveRideSummary(summary)
        }
    }

    /// Land a just-imported route at the top of Planned and in the library, so it survives a
    /// relaunch and can upload later.
    public func addImportedRoute(_ record: PlannedRouteRecord) {
        plannedRecords[record.id] = record
        library.savePlannedRoute(record)
        // A re-import that replaces an existing route keeps its device link, and its badge.
        refreshOnDeviceStates()
        routes.removeAll { $0.id == record.id }
        routes.insert(record.summary, at: 0)
        rebuildPlannedItems()
        tab = .planned
    }

    /// Land an end-to-end flipped copy at the top of Planned and leave the original alone, so
    /// the rider keeps both directions. The copy is a fresh library route: new id, no device
    /// link, uploads like any other. Returns nil when the source route has gone.
    @discardableResult
    public func reverseRoute(_ id: RouteID) -> RouteID? {
        guard let original = plannedRecords[id] else { return nil }
        let reversedRoute = original.route.reversed()
        // The header figures, as an import saves them.
        let totals = RouteObjectCodec.totals(points: reversedRoute.points)
        let distance = Double(totals?.distanceMeters ?? 0)
        let climb = Double(totals?.ascentMeters ?? 0)
        let name = RouteReversal.reversedName(original.summary.name)
        let newID = RouteID("reversed-\(UUID().uuidString.lowercased())")
        let summary = RouteSummary(
            id: newID,
            name: name,
            distanceMeters: distance,
            elevationGainMeters: climb,
            estimatedDuration: original.bikeType.estimatedDuration(distanceMeters: distance, ascentMeters: climb),
            pointCount: reversedRoute.points.count,
            source: original.summary.source,
            trackPreview: TrackPreview.normalizing(reversedRoute.points.map(\.coordinate))
        )
        // The source file is provenance only; nothing re-parses it to rebuild the geometry.
        let record = PlannedRouteRecord(
            summary: summary,
            route: reversedRoute,
            bikeType: original.bikeType,
            sourceFileName: original.sourceFileName,
            sourceFileData: original.sourceFileData,
            addedAt: now()
        )
        addImportedRoute(record)
        return newID
    }

    /// A saved planned route whose name matches case-insensitively, so the import edge can
    /// offer a replace instead of a duplicate.
    public func plannedRoute(named name: String) -> PlannedRouteRecord? {
        plannedRecords.values.plannedRoute(named: name)
    }

    public func isUploaded(_ id: RouteID) -> Bool { onDeviceState(id) != .notOnDevice }

    public func onDeviceState(_ id: RouteID) -> OnDeviceState { onDevice[id] ?? .notOnDevice }

    /// The kept detail, with the summary refreshed from the live list so a rename shows.
    public func importedDetail(for id: RouteID) -> RouteDetail? {
        guard let record = plannedRecords[id] else { return nil }
        var detail = record.detail()
        if let live = routes.first(where: { $0.id == id }) { detail.summary = live }
        return detail
    }

    /// The canonical parsed geometry a library-saved route re-encodes for upload. Nil for a
    /// device-listed route the phone never imported.
    public func plannedGeometry(for id: RouteID) -> ImportedRoute? {
        plannedRecords[id]?.route
    }

    public func plannedBikeType(for id: RouteID) -> BikeType {
        plannedRecords[id]?.bikeType ?? .road
    }

    /// A synced ride with its full tracklog, read from the store on demand: the interactive map,
    /// share and save as route use this, never the downsampled `trackPreview`. Nil when the ride
    /// carries no points, and the detail then degrades to the preview's coordinates.
    public func ride(_ id: RideID) -> Ride? {
        guard let summary = rides.first(where: { $0.id == id }),
            let points = library.ridePoints(id), !points.isEmpty
        else { return nil }
        return Ride(summary: summary, points: points)
    }

    /// The object id this planned route is stored under on the connected device, threaded into
    /// a re-upload so it replaces that object. A link minted on another device or in a previous
    /// era answers nil, so a replace can never overwrite an object the link does not point at.
    public func plannedDeviceObjectID(for id: RouteID) -> DeviceObjectID? {
        guard let link = plannedRecords[id]?.deviceLink, let scope = connectedScope,
            link.matches(scope)
        else { return nil }
        return link.objectID
    }

    /// The CRC the connected device is proven to hold, so the detail button reads "up to date"
    /// on the same proof the list badge uses, never on link presence alone.
    public func plannedProvenCommittedCRC(for id: RouteID) -> UInt32? {
        guard let record = plannedRecords[id] else { return nil }
        return provenCommittedCRC(for: record)
    }
    /// Show a new device name in the top bar at once. Settings owns the config write and the
    /// bond record.
    public func deviceRenamed(to name: String) {
        deviceName = name
    }

    /// Record the scope-qualified link an upload landed under, so the badge lights and a later
    /// re-upload replaces that object on that device. With no settled scope no link is recorded:
    /// the safe direction, because a scope-less link aliases across devices.
    public func markRouteUploaded(
        _ id: RouteID, objectID: DeviceObjectID, crc32: UInt32
    ) {
        markRouteUploaded(id, objectID: objectID, crc32: crc32, adopt: true)
    }

    /// A single route upload adopts: it pushes its trip object when the trip is on the device.
    /// A stage committed inside a whole-trip upload does not, because that queue pushes the trip
    /// object once at the end.
    func markRouteUploaded(
        _ id: RouteID, objectID: DeviceObjectID, crc32: UInt32, adopt: Bool
    ) {
        guard var record = plannedRecords[id] else { return }
        if let scope = connectedScope {
            record.deviceLink = DeviceRouteLink(scope: scope, objectID: objectID)
            record.uploadedCRC32 = crc32
            // The transfer verified this CRC for this object, so record it as device truth: the
            // badge proves before the next catalog read overwrites it.
            deviceRouteCRCs[objectID] = crc32

        } else {
            record.deviceLink = nil
            record.uploadedCRC32 = nil
        }
        plannedRecords[id] = record
        library.savePlannedRoute(record)
        refreshOnDeviceStates()
        // Adoption rule: a single route that belongs to a trip already on the device files into
        // the folder, so push the updated trip object. Runs after the route's own commit, so the
        // trip object carries the fresh stage id.
        if adopt { maybeAdoptRouteIntoDeviceTrip(id) }
    }

    /// Record the link and fingerprint a trip-object upload landed under, so the trip badge
    /// lights and a later push replaces that object in place. No scope or no id means no link,
    /// the safe direction.
    public func markTripUploaded(_ id: TripID, objectID: DeviceObjectID?, crc32: UInt32) {
        guard var trip = trip(id) else { return }
        if let scope = connectedScope, let objectID {
            trip.deviceLink = DeviceRouteLink(scope: scope, objectID: objectID)
            trip.uploadedCRC32 = crc32
            // The transfer verified this CRC, so the badge proves before the next `listTrips()`.
            deviceTripCRCs[objectID] = crc32
        } else {
            trip.deviceLink = nil
            trip.uploadedCRC32 = nil
        }
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Push the updated trip object when the route's trip is already on the device, so the
    /// newly-committed route files into the folder. Best-effort: a failed push leaves the trip
    /// page reading outdated, which the Upload-trip button or the next reconcile heals.
    private func maybeAdoptRouteIntoDeviceTrip(_ routeID: RouteID) {
        guard let scope = connectedScope,
            let tripID = tripContaining(routeID),
            let trip = trip(tripID),
            let link = trip.deviceLink, link.matches(scope),
            // Confirmed on the device: a non-zero CRC for the trip object, the same proof the
            // badge uses, so an in-session upload counts without re-reading.
            (deviceTripCRCs[link.objectID] ?? 0) != 0
        else { return }
        pushTripObject(tripID, replacing: link.objectID)
    }

    /// Encode and upload one trip object, replacing by id, and commit the link on success.
    /// Fire-and-forget; the reconcile is the backstop.
    private func pushTripObject(_ tripID: TripID, replacing objectID: DeviceObjectID?) {
        guard let trip = trip(tripID) else { return }
        let deviceStageIDs = currentTripDeviceStageIDs(for: trip)
        let payload = TripObjectCodec.encode(currentTripObject(for: trip, stageIDs: deviceStageIDs))
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

    /// The Tracked rows: exactly the rides the phone has synced, newest first. A ride on the
    /// device but not downloaded is deliberately absent, because it has only summary stats and a
    /// half-empty card is worse than none. Trashed rides stay in `rideSummaries` for Recover, so
    /// the trash filter is what hides them here.
    private func trackedList() -> [RideSummary] {
        rideSummaries.values
            .filter { !deletedRideIDs.contains($0.id) && trashedRideIDs[$0.id] == nil }
            .sorted { $0.date > $1.date }
    }

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
