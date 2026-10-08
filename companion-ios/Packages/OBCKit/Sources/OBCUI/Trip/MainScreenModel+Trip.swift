import Foundation
import OBCDomain
import OBCPlanner
import OBCTransport

@MainActor
extension MainScreenModel {
    // MARK: Trips

    /// Re-read trips and rebuild the interleaved Planned list. Every trip or route edit ends
    /// on this call.
    func reloadTrips() {
        trips = library.trips()
        rebuildPlannedItems()
    }

    func rebuildPlannedItems() {
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
        let tripSelf = OnDeviceState.determine(
            provenCommittedCRC: provenTripCommittedCRC(for: trip),
            currentCRC: { currentTripPayloadCRC(for: trip) }
        )
        return Self.composeTripState(tripSelf: tripSelf, dayStates: tripDayOnDeviceStates(id))
    }

    /// Each day route's copy on the connected device, in day order.
    public func tripDayOnDeviceStates(_ id: TripID) -> [OnDeviceState] {
        guard let trip = trip(id) else { return [] }
        return dayRoutes(of: trip).map { day in
            OnDeviceState.determine(provenCommittedCRC: provenDayCRC(trip, day: day.day), currentCRC: { day.crc32 })
        }
    }

    /// A day route's copy on the connected device. A copy made on another device or in another
    /// id era answers nil, so a replace never overwrites an object the link does not point at.
    func scopedDayCopy(_ trip: Trip, day: Int) -> TripDayCopy? {
        guard let scope = connectedScope, trip.dayCopies.indices.contains(day),
            let copy = trip.dayCopies[day], copy.link.matches(scope)
        else { return nil }
        return copy
    }

    /// The day-route twin of `provenCommittedCRC(for:)`.
    func provenDayCRC(_ trip: Trip, day: Int) -> UInt32? {
        guard let copy = scopedDayCopy(trip, day: day), let uploaded = copy.uploadedCRC32,
            let catalogCRC = deviceRouteCRCs[copy.link.objectID], catalogCRC != 0, catalogCRC == uploaded
        else { return nil }
        return uploaded
    }

    /// The trip object an upload would send now. Nil until every day route has a copy on the
    /// connected device: a trip object never names fewer days than the trip has.
    func currentTripObject(for trip: Trip) -> TripObjectCodec.Trip? {
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
    func dropDisprovedDayCopies(scope: LibraryScope, listed: Set<DeviceObjectID>) {
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
    func adoptDayCopiesByContent(
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
    func reconcileTripsOnDevice(with catalog: [TripCatalogEntry]) {
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
            library.saveTrip(trip)
            claimed.insert(entry.id)
        }
    }

    // MARK: Whole-trip upload

    public func planTripUpload(_ id: TripID) -> TripUploadPlan? {
        TripUploadModel.plan(id, in: self)
    }

    public func prepareTripUpload(
        _ id: TripID, timing: TripUploadModel.Timing = TripUploadModel.Timing()
    ) async -> TripUploadModel? {
        let upload = makeTripUploadModel(id, timing: timing)
        await upload?.prepare()
        return upload
    }

    public func makeTripUploadModel(
        _ id: TripID, timing: TripUploadModel.Timing = TripUploadModel.Timing()
    ) -> TripUploadModel? {
        TripUploadModel(tripID: id, main: self, timing: timing)
    }

    /// Pure and static, so the rule is testable without a device.
    static func composeTripState(
        tripSelf: OnDeviceState, dayStates: [OnDeviceState]
    ) -> OnDeviceState {
        guard !dayStates.isEmpty, tripSelf != .notOnDevice else { return .notOnDevice }
        if tripSelf == .upToDate, dayStates.allSatisfy({ $0 == .upToDate }) { return .upToDate }
        return .outdated
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

    /// The plan a trip opens with in the planner, under the trip's name and bike type. A trip
    /// saved without a plan opens as its kept line. Nil for a trip of more than
    /// ``PlannerPlan/maxDays`` days.
    public func tripPlan(for id: TripID) -> PlannerPlan? {
        guard let trip = trip(id), var plan = trip.plan ?? PlannerPlan.keptLine(trip) else { return nil }
        plan.name = trip.name
        if trip.plan == nil { plan.bike = RouteActivity(trip.bikeType).rawValue }
        return plan
    }

    /// Write a plan and the line planned from it into a trip. The id, key, name, dates, device
    /// links and journal stay; the days and the line come from the plan.
    public func saveTripChanges(_ id: TripID, plan: PlannerPlan, line: [RoutePoint], pointIndices: [Int]) {
        guard var trip = trip(id) else { return }
        trip.replacePlan(plan, line: line, pointIndices: pointIndices)
        saveEditedTrip(trip)
        nameDayEnds(id)
    }

    /// A new trip from a plan and the line planned from it. Nil when the line is empty.
    @discardableResult
    public func createTrip(
        name: String, plan: PlannerPlan, line: [RoutePoint], pointIndices: [Int], bikeType: BikeType
    ) -> TripID? {
        var trip = Trip(id: TripID(UUID().uuidString.lowercased()), name: name, bikeType: bikeType, addedAt: now())
        trip.replacePlan(plan, line: line, pointIndices: pointIndices)
        guard !trip.dayEnds.isEmpty else { return nil }
        library.saveTrip(trip)
        reloadTrips()
        nameDayEnds(trip.id)
        return trip.id
    }

    /// A route whose plan now has nights becomes a trip of the same name: the route leaves the
    /// library, and its device copy becomes the copy of day 1.
    @discardableResult
    public func replaceRouteWithTrip(
        _ id: RouteID, plan: PlannerPlan, line: [RoutePoint], pointIndices: [Int], bikeType: BikeType
    ) -> TripID? {
        guard let record = plannedRecords[id],
              let tripID = createTrip(name: record.summary.name, plan: plan, line: line, pointIndices: pointIndices, bikeType: bikeType)
        else { return nil }
        moveIntoTrip(record, tripID: tripID, day: 0)
        return tripID
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
    /// file is a day. `plan` is the plan of the files that are days.
    @discardableResult
    public func createTrip(
        name: String, files: [[RoutePoint]], dayNames: [String?] = [], waypoints: [[Waypoint]] = [],
        bikeType: BikeType? = nil, plan: PlannerPlan? = nil
    ) -> TripID? {
        let days = files.indices.filter { Trip.isDay(files[$0]) }
        guard !days.isEmpty else { return nil }
        guard days.count <= PlannerPlan.maxDays else { tripNotice = Self.tooManyDays; return nil }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        var trip = Trip.joining(
            days.map { files[$0] }, names: days.map { $0 < dayNames.count ? dayNames[$0] : nil },
            waypoints: days.map { $0 < waypoints.count ? waypoints[$0] : [] },
            id: TripID(UUID().uuidString.lowercased()),
            name: trimmed.isEmpty ? "New trip" : trimmed,
            bikeType: bikeType ?? lastBikeType.value, now: now())
        trip.plan = plan ?? PlannerPlan.keptLine(trip)
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
        guard trip.dayCount < PlannerPlan.maxDays else { tripNotice = Self.tooManyDays; return false }
        let plan = trip.plan
        tell(dropped: trip.append(file, name: name, waypoints: waypoints))
        trip.plan = plan?.appendingDay(file, name: name, waypoints: waypoints) ?? PlannerPlan.keptLine(trip)
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

    static let tooManyDays = "A trip has at most \(PlannerPlan.maxDays) days."

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
}

extension PlannedRouteRecord {
    /// The route as the join reads it. The caller keeps only routes with a line.
    fileprivate var joinFile: TripJoin.File {
        TripJoin.File(
            name: summary.name, start: route.points[0].coordinate,
            end: route.points[route.points.count - 1].coordinate)
    }
}
