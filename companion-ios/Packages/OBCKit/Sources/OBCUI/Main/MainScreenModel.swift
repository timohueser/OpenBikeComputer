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
    public private(set) var trips: [Trip] = []
    /// One line for the rider after a trip change that did not go as asked: a dropped day end,
    /// or a route too short to be a day. The screen clears it when shown.
    public var tripNotice: String?
    /// The Planned tab rows: trip cards and loose route cards, newest first. A route filed in
    /// a trip shows only inside that trip; `routes` keeps every planned summary.
    public private(set) var plannedItems: [PlannedItem] = []
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
    /// Each trip's day routes with the trip they were cut from.
    @ObservationIgnored private var tripDayCache: [TripID: (trip: Trip, days: [TripDayRoute])] = [:]
    /// Names the place at a coordinate, such as its locality. Nil in tests and previews.
    private let placeName: (@Sendable (Coordinate) async -> String?)?
    /// The trip reviews' place names, for the session.
    private let placeNameCache: PlaceNameCache?
    /// Finds stops near trip lines for the session. Nil in tests and previews.
    public let stopFinder: StopFinder?
    /// Routes to stops off a trip line and across gaps. Nil in tests and previews.
    private let legRouter: (any LegRouter)?
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
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        stopSearch: (any StopSearch)? = nil,
        legRouter: (any LegRouter)? = nil,
        now: @escaping () -> Date = Date.init
    ) {
        self.placeName = placeName
        self.legRouter = legRouter
        placeNameCache = placeName.map(PlaceNameCache.init(lookup:))
        self.stopFinder = stopSearch.map(StopFinder.init)
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
    private func catalogDisproves(
        _ objectID: DeviceObjectID, uploadedCRC32: UInt32?, listed: Set<DeviceObjectID>
    ) -> Bool {
        guard listed.contains(objectID) else { return true }
        let catalogCRC = deviceRouteCRCs[objectID] ?? 0
        return catalogCRC != 0 && uploadedCRC32 != nil && catalogCRC != uploadedCRC32
    }

    /// The first unclaimed catalog entry that holds `payload`, or holds it under the entry's own
    /// name: a rename moves the bytes without changing the route.
    private static func entry(
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

    public func trip(_ id: TripID) -> Trip? { trips.first { $0.id == id } }

    /// The trips a route can join, most recently edited first.
    public var tripPickerItems: [TripPickerItem] {
        trips.sorted { $0.editedAt > $1.editedAt }
            .map { TripPickerItem(id: $0.id, name: $0.name, dayCount: $0.dayCount) }
    }

    /// A trip's day routes as an upload sends them, with the names and the stats the device shows.
    public func tripDays(_ id: TripID) -> [TripDayRoute] {
        trip(id).map(dayRoutes(of:)) ?? []
    }

    public func tripStats(_ id: TripID) -> TripStats { TripStats(days: tripDays(id)) }

    /// The date of each day. A synced ride of a day moves the dates of the days after it.
    public func tripDayDates(_ id: TripID) -> [CivilDay?] {
        guard let trip = trip(id) else { return [] }
        var finished: [Int: CivilDay] = [:]
        for ride in rideSummaries.values.sorted(by: { $0.date < $1.date }) {
            guard let day = ride.trip, day.key == trip.key else { continue }
            finished[day.dayIndex] = CivilDay(ride.date)
        }
        return trip.dayDates(finished: finished)
    }

    /// The trip's date range for its card, when its days have dates.
    public func tripDateLine(_ id: TripID) -> String? {
        let dates = tripDayDates(id)
        guard let first = dates.first ?? nil, let last = dates.last ?? nil else { return nil }
        return OBCFormat.tripDates(first, last)
    }

    /// The cut and the encode run once per change of the line, the day ends or the bike type.
    private func dayRoutes(of trip: Trip) -> [TripDayRoute] {
        if let cached = tripDayCache[trip.id], cached.trip.line == trip.line,
            cached.trip.pieceStarts == trip.pieceStarts, cached.trip.dayEnds == trip.dayEnds,
            cached.trip.bikeType == trip.bikeType {
            return cached.days
        }
        let days = trip.dayRoutes()
        tripDayCache[trip.id] = (trip, days)
        return days
    }

    /// The trip badge: up to date only when the trip object itself is proven current and every
    /// day route is up to date. A trip the phone never pushed reads `.notOnDevice`.
    public func tripOnDeviceState(_ id: TripID) -> OnDeviceState {
        guard let trip = trip(id) else { return .notOnDevice }
        let days = dayRoutes(of: trip)
        let tripSelf = OnDeviceState.determine(
            provenCommittedCRC: provenTripCommittedCRC(for: trip),
            currentCRC: { currentTripPayloadCRC(for: trip) }
        )
        let dayStates = days.map { day in
            OnDeviceState.determine(provenCommittedCRC: provenDayCRC(trip, day: day.day), currentCRC: { day.crc32 })
        }
        return Self.composeTripState(tripSelf: tripSelf, dayStates: dayStates)
    }

    /// A day route's copy on the connected device. A copy made on another device or in another
    /// id era answers nil, so a replace never overwrites an object the link does not point at.
    private func scopedDayCopy(_ trip: Trip, day: Int) -> TripDayCopy? {
        guard let scope = connectedScope, trip.dayCopies.indices.contains(day),
            let copy = trip.dayCopies[day], copy.link.matches(scope)
        else { return nil }
        return copy
    }

    /// The day-route twin of `provenCommittedCRC(for:)`.
    private func provenDayCRC(_ trip: Trip, day: Int) -> UInt32? {
        guard let copy = scopedDayCopy(trip, day: day), let uploaded = copy.uploadedCRC32,
            let catalogCRC = deviceRouteCRCs[copy.link.objectID], catalogCRC != 0, catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    /// The trip object an upload would send now. Nil until every day route has a copy on the
    /// connected device: a trip object never names fewer days than the trip has.
    private func currentTripObject(for trip: Trip) -> TripObjectCodec.Trip? {
        let ids = (0..<trip.dayCount).compactMap { scopedDayCopy(trip, day: $0)?.link.objectID }
        guard !ids.isEmpty, ids.count == trip.dayCount else { return nil }
        return trip.tripObject(days: dayRoutes(of: trip), dayObjectIDs: ids)
    }

    /// The CRC of the trip object an upload would send now. A trip without a complete set of
    /// day copies has no object to compare, so it reads 0, which no committed CRC equals.
    private func currentTripPayloadCRC(for trip: Trip) -> UInt32 {
        currentTripObject(for: trip).map(TripObjectCodec.payloadCRC) ?? 0
    }

    /// The trip twin of `provenCommittedCRC(for:)`.
    private func provenTripCommittedCRC(for trip: Trip) -> UInt32? {
        guard let scope = connectedScope, let link = trip.deviceLink, link.matches(scope),
            let uploaded = trip.uploadedCRC32,
            let catalogCRC = deviceTripCRCs[link.objectID], catalogCRC != 0,
            catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    /// The day-route half of the route reconcile: drop each day copy the catalog disproves, with
    /// the planned-route rule.
    private func dropDisprovedDayCopies(scope: LibraryScope, listed: Set<DeviceObjectID>) {
        for var trip in library.trips() {
            var changed = false
            for (day, copy) in trip.dayCopies.enumerated() {
                guard let copy, copy.link.matches(scope),
                    catalogDisproves(copy.link.objectID, uploadedCRC32: copy.uploadedCRC32, listed: listed)
                else { continue }
                trip.dayCopies[day] = nil
                changed = true
            }
            if changed { library.saveTrip(trip) }
        }
    }

    /// `adoptByContent` for day routes: a day without a copy adopts an unclaimed catalog entry
    /// that holds its bytes, so a lost commit ack never mints a device twin.
    private func adoptDayCopiesByContent(
        scope: LibraryScope, catalog: [RouteCatalogEntry], claimed: inout Set<DeviceObjectID>
    ) {
        for var trip in library.trips().sorted(by: { $0.id.rawValue < $1.id.rawValue }) {
            var changed = false
            for day in dayRoutes(of: trip) where scopedDayCopy(trip, day: day.day) == nil {
                guard let entry = Self.entry(holding: day.payload, named: day.name, in: catalog, claimed: claimed)
                else { continue }
                while trip.dayCopies.count <= day.day { trip.dayCopies.append(nil) }
                trip.dayCopies[day.day] = TripDayCopy(
                    link: DeviceRouteLink(scope: scope, objectID: entry.id), uploadedCRC32: entry.crc32)
                claimed.insert(entry.id)
                changed = true
            }
            if changed { library.saveTrip(trip) }
        }
    }

    /// True up every trip's object link against the device trip catalog: drop pass, then adopt
    /// by content. A `crc32 = 0` entry proves nothing, so its link is kept intact and reads as
    /// outdated. Adoption also heals a lost commit ack, where the trip object landed but the phone
    /// never recorded it.
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
            trip.uploadedKey = nil
            library.saveTrip(trip)
        }
        adoptTripsByContent(scope: scope, catalog: catalog)
    }

    /// `adoptByContent`'s trip twin, with the same tie-breaks. The fingerprint is the trip
    /// object's CRC; a rename splices the catalog entry's name back in to rebuild the stored
    /// bytes.
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
            guard let object = currentTripObject(for: trip) else { continue }
            let currentCRC = TripObjectCodec.payloadCRC(object)
            guard let entry = adoptable.first(where: { entry in
                guard !claimed.contains(entry.id) else { return false }
                if entry.crc32 == currentCRC { return true }
                guard entry.name != trip.name else { return false }
                var renamed = object
                renamed.name = entry.name
                return entry.crc32 == TripObjectCodec.payloadCRC(renamed)
            }) else { continue }
            trip.deviceLink = DeviceRouteLink(scope: scope, objectID: entry.id)
            // The entry's CRC is what the device holds, so the trip reads as out of date and
            // the next send replaces by id.
            trip.uploadedCRC32 = entry.crc32
            // Both fingerprints the entry can match encode the trip's own key.
            trip.uploadedKey = trip.key
            library.saveTrip(trip)
            claimed.insert(entry.id)
        }
    }

    // MARK: Whole-trip upload

    /// Partition the day routes into skip, replace and fresh, and do the precheck math. Nil when
    /// the trip is gone.
    public func planTripUpload(_ id: TripID) -> TripUploadPlan? {
        guard let trip = trip(id) else { return nil }
        let dayInputs = dayRoutes(of: trip).map { day in
            TripUploadPlanner.DayInput(
                day: day.day,
                isUpToDate: provenDayCRC(trip, day: day.day) == day.crc32,
                committedObjectID: scopedDayCopy(trip, day: day.day)?.link.objectID
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
            days: dayInputs,
            tripObjectID: tripObjectID,
            deviceRouteCount: lastRouteCatalog?.count ?? 0,
            deviceTripCount: lastTripCatalog?.count ?? 0
        )
    }

    /// Re-read both catalogs and reconcile before planning: the retry-after-failure path. A
    /// day route, or the trip object, that committed but whose ack was lost would otherwise
    /// re-plan as fresh and mint a device twin. Routes before trips; the trip adoption reads
    /// the day copies.
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

    /// Turn the plan into a queue: a step per day route in ride order, then the trip object
    /// last. Nothing is sent when every day route and the trip object are already current. Each
    /// step commits its own link the instant it lands. Nil when the trip is gone.
    public func makeTripUploadModel(
        _ id: TripID, timing: TripUploadModel.Timing = TripUploadModel.Timing()
    ) -> TripUploadModel? {
        guard let trip = trip(id), let plan = planTripUpload(id) else { return nil }
        let days = dayRoutes(of: trip)
        var steps: [TripUploadModel.QueueStep] = []
        // A reversed trip has a new key. The device must not keep the old trip object, with the
        // old key's progress, over day routes that already hold the new days, so it goes first.
        if let link = trip.deviceLink, let scope = connectedScope, link.matches(scope),
            let uploadedKey = trip.uploadedKey, uploadedKey != trip.key {
            steps.append(.command(title: "Old trip details") { [weak self] in
                guard let self else { return }
                try await transport.deleteTrip(link.objectID)
                guard var trip = self.trip(id) else { return }
                trip.deviceLink = nil
                trip.uploadedCRC32 = nil
                trip.uploadedKey = nil
                library.saveTrip(trip)
                reloadTrips()
            })
        }
        for dayPlan in plan.days {
            let day = dayPlan.day
            let title = days[day].name
            switch dayPlan.action {
            case .skip:
                steps.append(.skip(title: title))
            case .fresh, .replace:
                let target: DeviceObjectID? =
                    if case .replace(let objectID) = dayPlan.action { objectID } else { nil }
                steps.append(.transfer(
                    title: title,
                    makeTransfer: { [weak self] in
                        guard let self, let blob = self.makeDayBlob(id, day: day, target: target) else { return nil }
                        return (self.transport.uploadRoute(blob), CRC32.checksum(blob.payload))
                    },
                    commit: { [weak self] objectID, crc in
                        guard let objectID else { return }
                        self?.markTripDayUploaded(id, day: day, objectID: objectID, crc32: crc)
                    }
                ))
            }
        }
        let tripProven = provenTripCommittedCRC(for: trip)
        let tripObjectUpToDate = tripProven != nil && tripProven == currentTripPayloadCRC(for: trip)
        if !(plan.allDaysSkip && tripObjectUpToDate) {
            steps.append(.transfer(
                title: "Trip details",
                makeTransfer: { [weak self] in
                    // The target is read at execution time: the old trip object may be gone.
                    guard let self, let blob = self.makeTripBlob(id) else { return nil }
                    return (self.transport.uploadTrip(blob), CRC32.checksum(blob.payload))
                },
                commit: { [weak self] objectID, crc in
                    self?.markTripUploaded(id, objectID: objectID, crc32: crc)
                    if objectID != nil { self?.deleteDroppedDayRoutes(id) }
                }
            ))
        }
        return TripUploadModel(
            transport: transport, tripName: trip.name, deviceName: deviceName,
            precheck: plan.precheck, steps: steps,
            timing: timing, activity: transferActivity
        )
    }

    /// Built at execution time from the current trip, so a change during the queue sends the
    /// bytes the trip holds now.
    private func makeDayBlob(_ tripID: TripID, day: Int, target: DeviceObjectID?) -> RouteBlob? {
        let days = tripDays(tripID)
        guard days.indices.contains(day), !days[day].payload.isEmpty else { return nil }
        return RouteBlob(
            summary: days[day].summary(tripID: tripID), payload: days[day].payload, targetObjectID: target)
    }

    /// Built at execution time, after the day routes committed, so it carries their fresh
    /// device ids. Nil until every day has a copy.
    private func makeTripBlob(_ tripID: TripID) -> TripBlob? {
        guard let trip = trip(tripID), let object = currentTripObject(for: trip) else { return nil }
        let target: DeviceObjectID? = {
            guard let link = trip.deviceLink, let scope = connectedScope, link.matches(scope) else { return nil }
            return link.objectID
        }()
        return TripBlob(
            name: trip.name, deviceStageIDs: object.days.map(\.routeID),
            payload: TripObjectCodec.encode(object), targetObjectID: target)
    }

    /// Pure and static, so the rule is testable without a device.
    static func composeTripState(
        tripSelf: OnDeviceState, dayStates: [OnDeviceState]
    ) -> OnDeviceState {
        guard !dayStates.isEmpty, tripSelf != .notOnDevice else { return .notOnDevice }
        if tripSelf == .upToDate, dayStates.allSatisfy({ $0 == .upToDate }) { return .upToDate }
        return .outdated
    }

    /// Record the link a day route upload landed under. No settled scope records no link.
    func markTripDayUploaded(_ id: TripID, day: Int, objectID: DeviceObjectID, crc32: UInt32) {
        guard var trip = trip(id) else { return }
        while trip.dayCopies.count <= day { trip.dayCopies.append(nil) }
        if let scope = connectedScope {
            trip.dayCopies[day] = TripDayCopy(
                link: DeviceRouteLink(scope: scope, objectID: objectID), uploadedCRC32: crc32)
            // The transfer verified this CRC, so the badge proves before the next catalog read.
            deviceRouteCRCs[objectID] = crc32
        } else {
            trip.dayCopies[day] = nil
        }
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Delete the device copies of days the trip no longer has. Runs after the new trip object
    /// landed, so no stored trip names them and the device keeps no orphan day routes. A failed
    /// delete leaves a plain route on the device.
    private func deleteDroppedDayRoutes(_ id: TripID) {
        guard var trip = trip(id), trip.dayCopies.count > trip.dayCount else { return }
        let dropped = trip.dayCopies[trip.dayCount...].compactMap { copy -> DeviceObjectID? in
            guard let copy, let scope = connectedScope, copy.link.matches(scope) else { return nil }
            return copy.link.objectID
        }
        trip.dayCopies.removeLast(trip.dayCopies.count - trip.dayCount)
        library.saveTrip(trip)
        reloadTrips()
        guard !dropped.isEmpty else { return }
        Task { [transport] in
            for objectID in dropped { try? await transport.deleteRoute(objectID) }
        }
    }

    /// Record the link and fingerprint a trip-object upload landed under, so the trip badge
    /// lights and a later push replaces that object in place. No scope or no id means no link,
    /// the safe direction.
    public func markTripUploaded(_ id: TripID, objectID: DeviceObjectID?, crc32: UInt32) {
        guard var trip = trip(id) else { return }
        if let scope = connectedScope, let objectID {
            trip.deviceLink = DeviceRouteLink(scope: scope, objectID: objectID)
            trip.uploadedCRC32 = crc32
            trip.uploadedKey = trip.key
            // The transfer verified this CRC, so the badge proves before the next `listTrips()`.
            deviceTripCRCs[objectID] = crc32
        } else {
            trip.deviceLink = nil
            trip.uploadedCRC32 = nil
            trip.uploadedKey = nil
        }
        library.saveTrip(trip)
        reloadTrips()
    }

    // MARK: Trip edits

    /// Save a rider's change to a trip. It becomes the most recently edited trip.
    private func saveEditedTrip(_ trip: Trip) {
        var trip = trip
        trip.editedAt = now()
        library.saveTrip(trip)
        reloadTrips()
    }

    /// Rename a trip. Phone-local; the new name rides the next trip upload.
    public func renameTrip(_ id: TripID, to name: String) {
        guard var trip = trip(id) else { return }
        trip.name = name
        saveEditedTrip(trip)
    }

    /// Give one day its own name. Nil or blank returns the day to "Day N ‹place›".
    public func renameTripDay(_ id: TripID, day: Int, to name: String?) {
        guard var trip = trip(id) else { return }
        trip.renameDay(day, to: name)
        saveEditedTrip(trip)
    }

    /// Reverse the trip in place: the direction and the day order. It gets a new trip key, so
    /// its device progress starts empty.
    public func reverseTrip(_ id: TripID) {
        guard var trip = trip(id) else { return }
        tell(dropped: trip.reverse())
        saveEditedTrip(trip)
        nameDayEnds(id)
    }

    /// Label the transfer after a day, or clear it. Phone-only: no upload carries it.
    public func setTripTransfer(_ id: TripID, day: Int, to kind: TransferKind?) {
        guard var trip = trip(id) else { return }
        trip.setTransfer(day, to: kind)
        saveEditedTrip(trip)
    }

    /// The model of one trip page's review. It reads the geocoder through the session's cache.
    public func tripJournal() -> TripJournalModel {
        var placeName: (@Sendable (Coordinate) async -> String?)?
        if let cache = placeNameCache {
            placeName = { await cache.name(at: $0) }
        }
        return TripJournalModel(library: library, placeName: placeName)
    }

    /// The stops of one day end, for the stops sheet. Nil for the last day, which ends at the line
    /// end.
    public func tripStops(_ id: TripID, day: Int, isOnline: Bool) -> TripStopsModel? {
        guard let trip = trip(id), day >= 0, day < trip.dayCount - 1 else { return nil }
        return TripStopsModel(trip: trip, day: day, finder: stopFinder, isOnline: isOnline) { [weak self] stop in
            self?.endTripDay(id, day: day, at: stop)
        }
    }

    /// Even out the days after `from` by riding time, snapped to the stops near the new ends:
    /// the trip review's offer. The days before `from` and every day end at a transfer stay.
    public func evenOutDays(_ id: TripID, from: Double) async {
        guard var trip = trip(id) else { return }
        let before = trip.dayEnds.map(\.distance)
        trip.evenOut(from: from, candidates: trip.place(trip.waypoints))
        let ends = trip.dayEnds.dropLast().map(\.distance).filter { $0 > from }
        let found = (try? await stopFinder?.stops(near: ends, on: trip.measuredLine)) ?? []
        // A move that landed during the search wins: the offer was for the day ends before it.
        // A place name written meanwhile does not count.
        guard var current = self.trip(id), current.dayEnds.map(\.distance) == before else { return }
        let candidates = current.place(current.waypoints) + zip(found, ends).flatMap { current.place($0, near: $1) }
        current.evenOut(from: from, candidates: candidates)
        saveEditedTrip(current)
        nameDayEnds(id)
    }

    /// The day editor of a trip. Done writes the draft's days into the trip as it is then, so an
    /// upload or a reconcile that ran meanwhile keeps its links.
    public func dayEditor(_ id: TripID, isSplitMode: Bool) -> TripDayEditorModel? {
        guard let trip = trip(id) else { return nil }
        return TripDayEditorModel(
            trip: trip, isSplitMode: isSplitMode, finder: stopFinder, router: legRouter, placeName: placeName
        ) { [weak self] edited in
            guard let self, var current = self.trip(id) else { return }
            current.replaceDays(from: edited)
            self.saveEditedTrip(current)
            self.nameDayEnds(id)
        }
    }

    /// End a day at a stop: the day end moves to the line point nearest the stop and takes its
    /// name.
    public func endTripDay(_ id: TripID, day: Int, at stop: PlacedStop) {
        guard var trip = trip(id), trip.endDay(day, at: stop) else { return }
        saveEditedTrip(trip)
    }

    /// Set or clear the date of Day 1.
    public func setTripStartDay(_ id: TripID, to day: CivilDay?) {
        guard var trip = trip(id) else { return }
        trip.startDay = day
        saveEditedTrip(trip)
    }

    /// Set a trip's bike type, and make it the type the next import starts with. Every day route
    /// carries the type, so the change out-dates them until the next upload.
    public func setTripBikeType(_ id: TripID, to type: BikeType) {
        lastBikeType.value = type
        guard var trip = trip(id) else { return }
        trip.bikeType = type
        saveEditedTrip(trip)
    }

    /// Delete a trip from the phone. While connected, also delete its day routes and its trip
    /// object on the device: the protocol delete does not cascade. Offline, the device copies stay
    /// and surface as loose routes there.
    public func deleteTrip(_ id: TripID) {
        guard let trip = trip(id) else { return }
        if let scope = connectedScope {
            let routeObjectIDs = trip.dayCopies.compactMap { copy -> DeviceObjectID? in
                guard let copy, copy.link.matches(scope) else { return nil }
                return copy.link.objectID
            }
            let tripObjectID: DeviceObjectID? = {
                guard let link = trip.deviceLink, link.matches(scope) else { return nil }
                return link.objectID
            }()
            if !routeObjectIDs.isEmpty || tripObjectID != nil {
                // Best-effort: a failed command leaves an orphan the reconcile heals.
                Task { [transport] in
                    if let tripObjectID { try? await transport.deleteTrip(tripObjectID) }
                    for objectID in routeObjectIDs { try? await transport.deleteRoute(objectID) }
                }
            }
        }
        tripDayCache[id] = nil
        library.deleteTrip(id)
        reloadTrips()
    }

    // MARK: Create & file

    /// The one join path: route files in ride order become one trip, one day per file, with
    /// the day ends on the file boundaries. A file shorter than a day adds nothing. Nil when no
    /// file is a day.
    @discardableResult
    public func createTrip(
        name: String, files: [[RoutePoint]], dayNames: [String?] = [], waypoints: [[Waypoint]] = [],
        bikeType: BikeType? = nil
    ) -> TripID? {
        let days = files.indices.filter { Trip.isDay(files[$0]) }
        guard !days.isEmpty else { return nil }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let trip = Trip.joining(
            days.map { files[$0] }, names: days.map { $0 < dayNames.count ? dayNames[$0] : nil },
            waypoints: days.map { $0 < waypoints.count ? waypoints[$0] : [] },
            id: TripID(UUID().uuidString.lowercased()),
            name: trimmed.isEmpty ? "New trip" : trimmed,
            bikeType: bikeType ?? lastBikeType.value, now: now())
        library.saveTrip(trip)
        reloadTrips()
        nameDayEnds(trip.id)
        return trip.id
    }

    /// Add a route file to a trip as its new last day. False when the file is too short to be a
    /// day, which changes nothing.
    @discardableResult
    public func appendToTrip(
        _ id: TripID, file: [RoutePoint], name: String? = nil, waypoints: [Waypoint] = []
    ) -> Bool {
        guard var trip = trip(id), Trip.isDay(file) else { return false }
        tell(dropped: trip.append(file, name: name, waypoints: waypoints))
        saveEditedTrip(trip)
        nameDayEnds(id)
        return true
    }

    /// Group routes into one trip, in the proposed join order. Each route moves into the trip
    /// as one day and leaves the library; its device copy becomes that day's copy, so the upload
    /// replaces it in place. A route too short to be a day stays a route.
    @discardableResult
    public func groupIntoTrip(_ routeIDs: [RouteID], name: String) -> TripID? {
        let records = routeIDs.compactMap { plannedRecords[$0] }
        let days = records.filter { Trip.isDay($0.route.points) }
        noteTooShort(records.filter { !Trip.isDay($0.route.points) }.map(\.summary.name))
        guard !days.isEmpty else { return nil }
        let ordered = TripJoin.proposedOrder(days.map(\.joinFile)).map { days[$0] }
        guard let tripID = createTrip(
            name: name, files: ordered.map(\.route.points), dayNames: ordered.map(\.summary.name),
            waypoints: ordered.map(\.route.waypoints), bikeType: ordered[0].bikeType)
        else { return nil }
        for (day, record) in ordered.enumerated() { moveIntoTrip(record, tripID: tripID, day: day) }
        return tripID
    }

    /// File a route per a picker selection: it becomes the last day of an existing trip, or the
    /// only day of a new one, and leaves the library. Returns the trip it went into, or nil when
    /// the route is too short to be a day and stays a route.
    @discardableResult
    public func fileRoute(_ routeID: RouteID, into selection: TripSelection) -> TripID? {
        guard let record = plannedRecords[routeID], selection != .none else { return nil }
        guard Trip.isDay(record.route.points) else {
            noteTooShort([record.summary.name])
            return nil
        }
        let tripID: TripID
        switch selection {
        case .none:
            return nil
        case .existing(let id):
            guard appendToTrip(
                id, file: record.route.points, name: record.summary.name, waypoints: record.route.waypoints)
            else { return nil }
            tripID = id
        case .new(let name):
            guard let id = createTrip(
                name: name, files: [record.route.points], dayNames: [record.summary.name],
                waypoints: [record.route.waypoints], bikeType: record.bikeType)
            else { return nil }
            tripID = id
        }
        moveIntoTrip(record, tripID: tripID, day: (trip(tripID)?.dayCount ?? 1) - 1)
        return tripID
    }

    /// The route leaves the library. Its device copy, if any, becomes the copy of `day`: the
    /// bytes differ, so the next upload replaces that object instead of leaving it an orphan.
    private func moveIntoTrip(_ record: PlannedRouteRecord, tripID: TripID, day: Int) {
        if let link = record.deviceLink, var trip = trip(tripID), day >= 0 {
            while trip.dayCopies.count <= day { trip.dayCopies.append(nil) }
            trip.dayCopies[day] = TripDayCopy(link: link, uploadedCRC32: record.uploadedCRC32)
            library.saveTrip(trip)
            reloadTrips()
        }
        deleteRoute(record.id)
    }

    /// The line the rider sees after a trip change that dropped day ends.
    private func tell(dropped: [DayEnd]) {
        guard !dropped.isEmpty else { return }
        tripNotice = dropped.map { end in
            "Day end \u{201C}\(end.name ?? "unnamed")\u{201D} was removed. It is no longer on the line."
        }.joined(separator: " ")
    }

    /// Tell the rider that these routes are too short to be days and stay routes.
    public func noteTooShort(_ names: [String]) {
        guard !names.isEmpty else { return }
        tripNotice = names.map { "\u{201C}\($0)\u{201D} is too short to be a day. It stays a route." }
            .joined(separator: " ")
    }

    /// Name each unnamed day end after its place, for the days without a name of their own. A
    /// lookup never replaces a name, and one that finds nothing leaves "Day N".
    private func nameDayEnds(_ id: TripID) {
        guard let placeName, let trip = trip(id) else { return }
        let unnamed = trip.dayEnds.enumerated()
            .filter { $0.element.name == nil && $0.element.title == nil }
            .map { ($0.offset, $0.element.coordinate) }
        guard !unnamed.isEmpty else { return }
        Task { [weak self] in
            for (day, coordinate) in unnamed {
                guard let name = await placeName(coordinate) else { continue }
                guard let self, var trip = self.trip(id), trip.dayEnds.indices.contains(day),
                    trip.dayEnds[day].coordinate == coordinate, trip.dayEnds[day].name == nil
                else { continue }
                trip.namePlace(day, to: name)
                library.saveTrip(trip)
                reloadTrips()
            }
        }
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
        // An edited ride's id can be a synced ride's id that another edit still shows.
        if !library.isEditedRide(id) { library.markRideSynced(id) }
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
            // The store marks deleted only the synced rides that no other edit still shows.
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

    /// Whether Revert to original applies: the ride is a trim, a split part or a merge.
    public func isEditedRide(_ id: RideID) -> Bool { library.isEditedRide(id) }

    /// Keep only the part of the ride inside `range`. False when fewer than two points remain.
    @discardableResult
    public func trimRide(_ id: RideID, to range: ClosedRange<Date>) -> Bool {
        guard let summary = rideSummaries[id], library.trimRide(id, to: range, summary: summary)
        else { return false }
        reloadRides()
        return true
    }

    /// Make two rides of one at `time`. Returns the second ride's id.
    @discardableResult
    public func splitRide(_ id: RideID, at time: Date) -> RideID? {
        guard let summary = rideSummaries[id], let second = library.splitRide(id, at: time, summary: summary)
        else { return nil }
        reloadRides()
        return second
    }

    /// The listed ride after `id` in time: the partner of Merge with next.
    public func nextRide(after id: RideID) -> RideSummary? {
        guard let ride = rides.first(where: { $0.id == id }) else { return nil }
        return rides.filter { $0.date > ride.date }.min { $0.date < $1.date }
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

    /// Restore the synced rides behind this edited ride, as they were synced. A part of the edit
    /// in Recently Deleted leaves it: the restored rides show in the list.
    public func revertRide(_ id: RideID) {
        for released in library.revertRide(id) where trashedRideIDs[released] != nil {
            trashedRideIDs[released] = nil
            library.unmarkRideTrashed(released)
        }
        reloadRides()
    }

    /// The next ride, when the two look like one ride with a break, do not come from the same
    /// synced ride, and the rider has not dismissed the merge. The tracklogs decode off the main
    /// actor.
    public func mergeSuggestion(for id: RideID) async -> RideSummary? {
        guard let current = rides.first(where: { $0.id == id }), let next = nextRide(after: id),
              !library.dismissedMerges().contains(RidePair(first: id, second: next.id)),
              library.rideSources(id).isDisjoint(with: library.rideSources(next.id))
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

extension PlannedRouteRecord {
    /// The route as the join reads it. The caller keeps only routes with a line.
    fileprivate var joinFile: TripJoin.File {
        TripJoin.File(
            name: summary.name, start: route.points[0].coordinate,
            end: route.points[route.points.count - 1].coordinate)
    }
}
