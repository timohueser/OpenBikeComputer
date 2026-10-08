import Foundation
import Observation
import OBCDomain
import OBCPlanner
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

    public private(set) var deviceName = DeviceInfo.unnamed
    public private(set) var connection: ConnectionState = .connecting
    public private(set) var battery: Int?
    public private(set) var loadState: LoadState = .loading
    public internal(set) var routes: [RouteSummary] = []
    /// Each planned route's proven device-copy state, behind the list badge. Observable, so
    /// the badge moves the instant an upload commits or a re-import changes the content.
    public private(set) var onDevice: [RouteID: OnDeviceState] = [:]
    /// Every saved trip, newest first.
    public internal(set) var trips: [Trip] = []
    /// One line for the rider after a trip change that did not go as asked: a dropped day end,
    /// or a route too short to be a day. The screen clears it when shown.
    public var tripNotice: String?
    /// The Planned tab rows: trip cards and loose route cards, newest first. A route filed in
    /// a trip shows only inside that trip; `routes` keeps every planned summary.
    public internal(set) var plannedItems: [PlannedItem] = []
    public private(set) var rides: [RideSummary] = [] {
        didSet { rideLibrary.rides = rides }
    }
    /// Counts ride edits. A ride detail rebuilds on a change, so it shows the edited ride.
    public private(set) var rideEditCount = 0
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
    public let rideLibrary: RideLibraryModel

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

    /// The Tracked rows: the library's year and bike-type filter, then the search.
    public var filteredRides: [RideSummary] {
        filtered(rideLibrary.filteredRides, by: \.name)
    }

    // MARK: Wiring

    let transport: any DeviceLink & DeviceBattery & DeviceObjects & DeviceClock
    let library: any LibraryStore
    let lastBikeType: LastBikeTypeStore
    /// Runs once per established connection. Nil in tests and previews skips it.
    private let nameReconciler: DeviceNameReconciler?
    /// Device rides deleted on the phone. The merge hides them so a sync cannot restore them.
    @ObservationIgnored private var deletedRideIDs: Set<RideID> = []
    /// Rides in Recently Deleted, with the time each was trashed.
    @ObservationIgnored private var trashedRideIDs: [RideID: Date] = [:]
    @ObservationIgnored private(set) var plannedRecords: [RouteID: PlannedRouteRecord] = [:]
    /// Ride summaries only. Tracklogs stay on disk and load one ride at a time through
    /// `ride(_:)`.
    @ObservationIgnored private(set) var rideSummaries: [RideID: RideSummary] = [:]
    @ObservationIgnored private var started = false
    @ObservationIgnored private var connectionRevision = 0
    @ObservationIgnored private var streamTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var loadTask: Task<Void, Never>?
    /// Set when a reload is requested while `loadTask` is already reading. Bursts coalesce
    /// here instead of cancelling a live transfer.
    @ObservationIgnored private var reloadRequested = false
    /// The last route catalog a reload read. On launch the catalog usually arrives before the
    /// identity verdict, so the reconcile re-runs once the scope settles.
    @ObservationIgnored var lastRouteCatalog: [RouteCatalogEntry]?
    /// Per-object content CRCs from the device route catalog. A link is a checkmark only when
    /// this holds a non-zero CRC for its object equal to the record's committed fingerprint.
    /// Zero, or an absent key, proves nothing.
    @ObservationIgnored var deviceRouteCRCs: [DeviceObjectID: UInt32] = [:]
    /// The last trip catalog a reload read, the trip sibling of `lastRouteCatalog`.
    @ObservationIgnored var lastTripCatalog: [TripCatalogEntry]?
    /// Per-trip content CRCs from the device trip catalog, the trip half of `deviceRouteCRCs`.
    @ObservationIgnored var deviceTripCRCs: [DeviceObjectID: UInt32] = [:]
    /// Each trip's day routes with the trip they were cut from.
    @ObservationIgnored var tripDayCache: [TripID: (trip: Trip, days: [TripDayRoute])] = [:]
    /// Names the place at a coordinate, such as its locality. Nil in tests and previews.
    let placeName: (@Sendable (Coordinate) async -> String?)?
    /// The trip reviews' place names, for the session.
    let placeNameCache: PlaceNameCache?
    /// The in-flight transfer ledger. Nil in tests and previews.
    @ObservationIgnored let transferActivity: TransferActivity?
    /// Reconnect waits for the previous identity read to drain before starting another.
    @ObservationIgnored private(set) var identityTask: Task<Void, Never>?

    public static let trashRetentionDays = 30

    let now: () -> Date

    public init(
        transport: any DeviceLink & DeviceBattery & DeviceObjects & DeviceClock,
        library: any LibraryStore = InMemoryLibraryStore(),
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore(),
        syncTiming: RideSyncCoordinator.Timing = RideSyncCoordinator.Timing(),
        nameReconciler: DeviceNameReconciler? = nil,
        transferActivity: TransferActivity? = nil,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        now: @escaping () -> Date = Date.init
    ) {
        self.placeName = placeName
        placeNameCache = placeName.map(PlaceNameCache.init(lookup:))
        self.transport = transport
        self.library = library
        self.lastBikeType = lastBikeType
        self.nameReconciler = nameReconciler
        self.transferActivity = transferActivity
        self.now = now
        self.rideLibrary = RideLibraryModel(library: library, now: now)
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
            // A synced ride under an edit shows through the edit, never as a row of its own.
            guard !library.rideViews().contains(where: { $0.sources.contains(ride.id) }) else {
                return reloadRides()
            }
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
                guard state != previous else { continue }
                previous = state
                connectionRevision += 1
                if state == .connected {
                    identityChecked = false
                    connectedScope = nil
                    lastRouteCatalog = nil
                    lastTripCatalog = nil
                    deviceRouteCRCs = [:]
                    deviceTripCRCs = [:]
                    refreshOnDeviceStates()
                    // Initial connection and reconnect share the same catalog/identity order.
                    reload()
                    let catalogs = loadTask
                    let priorIdentity = identityTask
                    let revision = connectionRevision
                    identityTask = Task { [weak self] in
                        await priorIdentity?.value
                        await catalogs?.value
                        guard let self, connection == .connected,
                              connectionRevision == revision, !Task.isCancelled else { return }
                        await runIdentityCheck()
                        guard connection == .connected, connectionRevision == revision else { return }
                        await nameReconciler?.reconcile()
                    }
                } else {
                    reloadRequested = false
                    loadState = .loaded
                }
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
        guard connection == .connected, protocolMismatch == nil else {
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
        while reloadRequested, connection == .connected, !Task.isCancelled {
            reloadRequested = false
            let revision = connectionRevision
            do {
                // Only the route catalog: Tracked is library-first, so its rows come from the
                // local library and device rides are pulled by Sync alone.
                let deviceRoutes = try await transport.listRoutes()
                guard !Task.isCancelled else { break }
                guard connectionRevision == revision else { continue }
                lastRouteCatalog = deviceRoutes
                reconcileOnDevice(with: deviceRoutes)
                // Fail closed: a failed trip read skips the reconcile, because treating it as
                // "zero trips" drops every trip link, and the next upload then mints a
                // duplicate device trip instead of replacing in place.
                if let deviceTrips = try? await transport.listTrips() {
                    guard !Task.isCancelled else { break }
                    guard connectionRevision == revision else { continue }
                    lastTripCatalog = deviceTrips
                    reconcileTripsOnDevice(with: deviceTrips)
                }
                guard connectionRevision == revision else { continue }
                routes = plannedList()
                reloadTrips()
                rides = trackedList()
                loadState = .loaded
            } catch {
                guard !Task.isCancelled else { break }
                guard connectionRevision == revision else { continue }
                loadState = .failed
            }
        }
        loadTask = nil
    }

    /// Read identity before scope-filtered reconciliation.
    /// A missing scope or failed read leaves device writes disabled until the
    /// next successful connection. Local library browsing remains available.
    private func runIdentityCheck() async {
        guard connection == .connected else { return }
        let revision = connectionRevision
        let wasIncompatible = protocolMismatch != nil
        // Unknown until proven, every connection: the device may have been
        // reinitialized (new StoreId) or swapped since the last read.
        connectedScope = nil
        await stampDeviceClock()
        guard connectionRevision == revision, !Task.isCancelled else { return }
        let info = try? await transport.deviceInfo()
        guard connectionRevision == revision, !Task.isCancelled else { return }
        if let info {
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
            if wasIncompatible { reload() }
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
    func reconcileOnDevice(with deviceRoutes: [RouteCatalogEntry]) {
        // Refreshed wholesale from the device, replacing any optimistic post-upload poke.
        deviceRouteCRCs = Dictionary(
            deviceRoutes.map { ($0.id, $0.crc32) }, uniquingKeysWith: { first, _ in first })
        guard let scope = connectedScope else {
            refreshOnDeviceStates()
            return
        }
        let listed = Set(deviceRoutes.map(\.id))
        // 1) Drop links the catalog disproves.
        for (id, var record) in plannedRecords {
            guard let link = record.deviceLink, link.matches(scope),
                catalogDisproves(link.objectID, uploadedCRC32: record.uploadedCRC32, listed: listed)
            else { continue }
            record.deviceLink = nil
            record.uploadedCRC32 = nil
            plannedRecords[id] = record
            library.savePlannedRoute(record)
        }
        dropDisprovedDayCopies(scope: scope, listed: listed)
        // 2) Adopt by content: heal identical unlinked copies without a re-upload. Planned
        //    routes and day routes share the route catalog, so they share the claimed ids.
        var claimed = Set(plannedRecords.values.compactMap { record -> DeviceObjectID? in
            guard let link = record.deviceLink, link.matches(scope) else { return nil }
            return link.objectID
        })
        for trip in library.trips() {
            for case let copy? in trip.dayCopies where copy.link.matches(scope) { claimed.insert(copy.link.objectID) }
        }
        adoptByContent(scope: scope, catalog: deviceRoutes, claimed: &claimed)
        adoptDayCopiesByContent(scope: scope, catalog: deviceRoutes, claimed: &claimed)
        refreshOnDeviceStates()
    }

    /// The catalog disproves a link when its object is absent, or present with a non-zero CRC
    /// that differs from the committed fingerprint. A `crc32 = 0` entry proves nothing, so that
    /// link is kept.
    func catalogDisproves(
        _ objectID: DeviceObjectID, uploadedCRC32: UInt32?, listed: Set<DeviceObjectID>
    ) -> Bool {
        guard listed.contains(objectID) else { return true }
        let catalogCRC = deviceRouteCRCs[objectID] ?? 0
        return catalogCRC != 0 && uploadedCRC32 != nil && catalogCRC != uploadedCRC32
    }

    /// The first unclaimed catalog entry that holds `payload`, or holds it under the entry's own
    /// name: a rename moves the bytes without changing the route.
    static func entry(
        holding payload: Data, named name: String, in catalog: [RouteCatalogEntry],
        claimed: Set<DeviceObjectID>
    ) -> RouteCatalogEntry? {
        let crc = CRC32.checksum(payload)
        return catalog.first { entry in
            guard entry.crc32 != 0, !claimed.contains(entry.id) else { return false }
            if entry.crc32 == crc { return true }
            guard entry.name != name else { return false }
            return entry.crc32 == CRC32.checksum(RouteObjectCodec.renamed(payload, to: entry.name))
        }
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
    private func adoptByContent(
        scope: LibraryScope, catalog: [RouteCatalogEntry], claimed: inout Set<DeviceObjectID>
    ) {
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
            guard let entry = Self.entry(holding: payload, named: record.summary.name, in: catalog, claimed: claimed)
            else { continue }
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

    func plannedList() -> [RouteSummary] {
        plannedRecords.values.sorted { $0.addedAt > $1.addedAt }.map(\.summary)
    }

    // MARK: Delete

    /// Remove a planned route from the phone. Never from the device: a copy already there
    /// stays, mirroring the ride rule in reverse.
    public func deleteRoute(_ id: RouteID) {
        routes.removeAll { $0.id == id }
        plannedRecords[id] = nil
        onDevice[id] = nil
        library.deletePlannedRoute(id)
        rebuildPlannedItems()
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
        let edited = library.isEditedRide(id)
        library.deleteRide(id)
        if edited {
            // The store marks the edit's synced rides deleted.
            deletedRideIDs = library.deletedRideIDs()
        } else {
            deletedRideIDs.insert(id)
            library.markRideDeleted(id)
        }
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

    // MARK: Ride edits

    /// Whether Revert to original applies: the ride is a trim or a merge.
    public func isEditedRide(_ id: RideID) -> Bool { library.isEditedRide(id) }

    /// Keep only the part of the ride inside `range`. False when fewer than two points remain.
    @discardableResult
    public func trimRide(_ id: RideID, to range: ClosedRange<Date>) -> Bool {
        guard let summary = rideSummaries[id], library.trimRide(id, to: range, summary: summary)
        else { return false }
        reloadRides()
        return true
    }

    /// The listed ride after `id` in time: the partner of Merge with next.
    public func nextRide(after id: RideID) -> RideSummary? {
        guard let ride = rides.first(where: { $0.id == id }) else { return nil }
        return rides.filter { $0.date > ride.date && $0.isDemo == ride.isDemo }.min { $0.date < $1.date }
    }

    /// Join the next ride onto this one. The time between the two does not count as moving time.
    @discardableResult
    public func mergeRideWithNext(_ id: RideID) -> Bool {
        guard let summary = rideSummaries[id], let next = nextRide(after: id),
              library.mergeRides(summary, next.id)
        else { return false }
        reloadRides()
        return true
    }

    /// Restore the synced rides behind this edited ride, as they were synced.
    public func revertRide(_ id: RideID) {
        library.revertRide(id)
        reloadRides()
    }

    /// The next ride, when the two look like one ride with a break and the rider has not
    /// dismissed the merge. The tracklogs decode off the main actor.
    public func mergeSuggestion(for id: RideID) async -> RideSummary? {
        guard let current = rides.first(where: { $0.id == id }), let next = nextRide(after: id),
              !library.dismissedMerges().contains(RidePair(first: id, second: next.id))
        else { return nil }
        let library = library
        let suggests = await Task.detached(priority: .utility) {
            guard let first = library.ridePoints(id), let second = library.ridePoints(next.id) else { return false }
            return RideEdit.suggestsMerge(Ride(summary: current, points: first), Ride(summary: next, points: second))
        }.value
        return suggests ? next : nil
    }

    public func dismissMergeSuggestion(for id: RideID) {
        guard let next = nextRide(after: id) else { return }
        library.dismissMerge(RidePair(first: id, second: next.id))
    }

    /// An edit can add, remove or restore rides, so the whole list reloads.
    private func reloadRides() {
        rideEditCount += 1
        rideSummaries = Dictionary(uniqueKeysWithValues: library.rideSummaries().map { ($0.id, $0) })
        rides = trackedList()
        trashedRides = trashedList()
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

    /// Replace a route's line and plan in place. The id, name, source file and device link stay, so
    /// a copy on the device reads out of date until the next upload.
    public func saveRouteChanges(_ id: RouteID, to edited: PlannedRouteRecord) {
        guard let old = plannedRecords[id] else { return }
        var record = edited
        record.summary = RouteSummary(
            id: id, name: old.summary.name, distanceMeters: edited.summary.distanceMeters,
            elevationGainMeters: edited.summary.elevationGainMeters, estimatedDuration: edited.summary.estimatedDuration,
            pointCount: edited.summary.pointCount, source: edited.summary.source, trackPreview: edited.summary.trackPreview)
        record.sourceFileName = old.sourceFileName
        record.sourceFileData = old.sourceFileData
        record.deviceLink = old.deviceLink
        record.uploadedCRC32 = old.uploadedCRC32
        record.addedAt = old.addedAt
        plannedRecords[id] = record
        library.savePlannedRoute(record)
        if let index = routes.firstIndex(where: { $0.id == id }) { routes[index] = record.summary }
        routeEditCount += 1
        refreshOnDeviceStates()
        rebuildPlannedItems()
    }

    /// Counts the saved route changes, so a route page builds again on its new line.
    public private(set) var routeEditCount = 0

    /// The plan a saved route opens with in the planner, under the route's name and bike type. A
    /// route saved without a plan opens as its kept line.
    public func plannedPlan(for id: RouteID) -> PlannerPlan? {
        guard let record = plannedRecords[id],
            var plan = record.plan ?? PlannerPlan.keptLine(record.route.points, waypoints: record.route.waypoints)
        else { return nil }
        plan.name = record.summary.name
        if record.plan == nil { plan.bike = RouteActivity(record.bikeType).rawValue }
        return plan
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

    /// The file a saved route was imported from, for the line under its title.
    public func plannedSourceFileName(for id: RouteID) -> String? {
        plannedRecords[id]?.sourceFileName
    }

    public func plannedBikeType(for id: RouteID) -> BikeType {
        plannedRecords[id]?.bikeType ?? .road
    }

    /// A synced ride with its full tracklog, read from the store on demand: the interactive map,
    /// the profile, share and save as route use this, never the downsampled `trackPreview`. Nil
    /// when the ride carries no points, and the detail then degrades to the preview's coordinates.
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
    public func markRouteUploaded(_ id: RouteID, objectID: DeviceObjectID, crc32: UInt32) {
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
