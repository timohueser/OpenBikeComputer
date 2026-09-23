import SwiftUI
import Observation
import OBCDomain

/// The figures of the days a drag in flight changes, published apart from the editor model so
/// a frame re-renders the two rows that read them and nothing else.
@MainActor @Observable
public final class LiveDayFigures {
    /// By day index; empty between drags.
    public internal(set) var days: [Int: DayStats] = [:]
}

/// The state under the day editor: a draft of the trip, the handle control that moves its day
/// ends, the day figures, and one undo step per change. Views drive it and Done hands the
/// draft back through `onSave`; nothing is saved before.
///
/// A drag is live in ``live`` on every frame and reaches the trip once, when the finger lets
/// go. Balancing runs twice per tap: at once with the stops already known, then again when
/// Apple Maps answers for the new day ends, so the stepper responds under the finger and the
/// snap follows a moment later.
@MainActor @Observable
public final class TripDayEditorModel {
    public private(set) var trip: Trip
    /// The day-end handles on the map and the profile. Colours follow the day index.
    public let handles: LineMarkerEditorModel
    /// The figures of every day for the day ends as committed.
    public private(set) var stats: [DayStats] = []
    /// The figures of the days under a drag, per frame.
    public let live = LiveDayFigures()
    /// Per day, why its end cannot be removed; nil when it can. Worked out once per change, so
    /// a drag frame costs no line walk.
    public private(set) var removeBlockers: [String?] = []
    /// The day whose row is highlighted: the last one tapped or dragged.
    public private(set) var selectedDay: Int?
    /// One long file became this trip: a stepper sets the day count.
    public let isSplitMode: Bool
    /// The trips before each committed change, newest last.
    private var undone: [Trip] = []

    private let original: Trip
    private let line: MeasuredLine
    private let finder: StopFinder?
    private let placeName: (@Sendable (Coordinate) async -> String?)?
    private let onSave: (Trip) -> Void
    /// The second, snapping pass of the last balance. A new balance cancels it.
    @ObservationIgnored var snapTask: Task<Void, Never>?
    /// One handle id per interior day end. Ids outlive moves and re-balances, so a handle
    /// animates to its new place instead of being replaced.
    @ObservationIgnored private var handleIDs: [Int] = []
    @ObservationIgnored private var nextHandleID = 1

    /// `nil` for a trip whose line has no positions to edit.
    public init?(
        trip: Trip, isSplitMode: Bool, finder: StopFinder?,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        onSave: @escaping (Trip) -> Void
    ) {
        let line = trip.measuredLine
        guard let handles = LineMarkerEditorModel(line: line, markers: [], segmentColors: [.clear]) else { return nil }
        self.trip = trip
        self.original = trip
        self.line = line
        self.handles = handles
        self.isSplitMode = isSplitMode
        self.finder = finder
        self.placeName = placeName
        self.onSave = onSave
        handles.onEvent = { [weak self] event in self?.handle(event) }
        handles.onTap = { [weak self] id in self?.select(self?.day(of: id)) }
        handles.onCloseUp = { [weak self] stretch in self?.loadStops(along: stretch) }
        handles.stopActionTitle = { [weak self] stop in
            self?.trip.day(thatCanEndAt: stop).map { "End Day \($0 + 1) here" }
        }
        handles.onStopAction = { [weak self] stop in self?.endDay(at: stop) }
        handles.stops = trip.place(trip.waypoints)
        syncHandles()
    }

    // MARK: Reading

    public var hasChanges: Bool { trip != original }
    public var canUndo: Bool { !undone.isEmpty }
    /// The line has no elevation, so days are balanced by distance alone.
    public var balancesByDistance: Bool { !line.hasElevation }
    /// "~5 h a day": the riding time of the whole trip over its days.
    public var averageDayDuration: TimeInterval {
        line.cost(to: line.length, bikeType: trip.bikeType) / Double(max(trip.dayCount, 1))
    }
    /// Why `day`'s end cannot be removed, or nil when it can.
    public func removeBlocker(_ day: Int) -> String? {
        removeBlockers.indices.contains(day) ? removeBlockers[day] : nil
    }
    /// The most days the stepper offers for this line.
    public var maxDays: Int { Trip.maxSplitDays(forLength: line.length) }
    /// The day the handle belongs to.
    public func day(of handle: LineMarker.ID) -> Int? { handleIDs.firstIndex(of: handle) }
    /// The day is long enough to hold a second day end.
    public func canSplit(_ day: Int) -> Bool {
        guard trip.dayEnds.indices.contains(day) else { return false }
        let from = day > 0 ? trip.dayEnds[day - 1].distance : 0
        return trip.dayEnds[day].distance - from >= 2 * Trip.minimumDayMeters
    }

    // MARK: Selection

    /// Highlight `day`'s row. It moves nothing: the map and the profile stay where they are.
    public func select(_ day: Int?) {
        selectedDay = day
    }

    /// The stops near every day end, the candidates of a balance. Asked once when the editor
    /// opens; a balance asks again for its new ends.
    public func loadStops() {
        let ends = trip.dayEnds.dropLast().map(\.distance)
        Task { [weak self] in _ = await self?.candidates(near: ends) }
    }

    /// Ask for stops along `stretch` at fixed points of the line, so a map panned back and forth
    /// asks each point once.
    func loadStops(along stretch: ClosedRange<Double>) {
        let grid = Self.stopGridMeters
        let points = stride(from: (stretch.lowerBound / grid).rounded(.down) * grid, through: stretch.upperBound, by: grid)
        let asked = Array(points)
        Task { [weak self] in _ = await self?.candidates(near: asked) }
    }

    /// Two grid points are this far apart along the line: every stop within
    /// ``DayBalance/snapOffsetMeters`` of the line lies within ``StopFinder/radiusMeters`` of one.
    static let stopGridMeters = 2 * (StopFinder.radiusMeters - DayBalance.snapOffsetMeters)

    // MARK: Changes

    /// Split mode: cut the line into `count` days. Outside split mode the count changes only
    /// with Split and Join, so a stepper there does nothing.
    public func setDayCount(_ count: Int) {
        guard isSplitMode else { return }
        let days = min(max(count, 1), maxDays)
        balance { trip, candidates in trip.split(into: days, candidates: candidates) }
    }

    /// Cut `day` in two at the middle of its riding time. The new end closes `day`, and the
    /// rest of the old day becomes the next one.
    public func splitDay(_ day: Int) {
        settle()
        guard canSplit(day) else { return }
        let from = day > 0 ? trip.dayEnds[day - 1].distance : 0
        let middle = (line.cost(to: from, bikeType: trip.bikeType) + line.cost(to: trip.dayEnds[day].distance, bikeType: trip.bikeType)) / 2
        let cut = line.distance(atCost: middle, bikeType: trip.bikeType)
        var added: Int?
        commit { trip in
            added = trip.addDayEnd(at: cut)
            return added != nil
        }
        guard let added else { return }
        handleIDs.insert(takeHandleID(), at: added)
        syncHandles()
        selectedDay = added
    }

    /// Join `day` with the next one: its end goes.
    public func joinDay(_ day: Int) {
        settle()
        guard commit({ $0.removeDayEnd(day) }) else { return }
        handleIDs.remove(at: day)
        if let selected = selectedDay, selected >= day { selectedDay = selected == day ? nil : selected - 1 }
        syncHandles()
    }

    public func renameDay(_ day: Int, to title: String?) {
        commit { trip in
            trip.renameDay(day, to: title)
            return true
        }
    }

    /// End the day nearest `stop` there: the map callout's action.
    public func endDay(at stop: PlacedStop) {
        settle()
        guard let day = trip.day(thatCanEndAt: stop) else { return }
        commit { $0.endDay(day, at: stop) }
        syncHandles()
    }

    /// The stops near `day`'s end, for the stops sheet. A pick ends the day at the stop.
    public func stops(for day: Int, isOnline: Bool) -> TripStopsModel? {
        guard day >= 0, day < trip.dayCount - 1 else { return nil }
        return TripStopsModel(trip: trip, day: day, finder: finder, isOnline: isOnline) { [weak self] stop in
            self?.commit { $0.endDay(day, at: stop) }
            self?.syncHandles()
        }
    }

    public func undo() {
        settle()
        guard let previous = undone.popLast() else { return }
        snapTask?.cancel()
        trip = previous
        if let day = selectedDay, day >= trip.dayCount - 1 { selectedDay = nil }
        syncHandles()
    }

    public func save() {
        settle()
        snapTask?.cancel()
        onSave(trip)
    }

    // MARK: Mechanics

    /// A finger still on a handle lifts first: its move commits before any other change, so
    /// the change applies to the day ends the rider sees.
    private func settle() {
        handles.end()
    }

    private func handle(_ event: LineMarkerEvent) {
        switch event {
        case .began(let id):
            selectedDay = day(of: id)
        case .moved(let id, _):
            // Only the day that ends here and the day after it change.
            guard let day = day(of: id) else { return }
            let all = line.dayStats(ends: handles.markers.map(\.distance), bikeType: trip.bikeType)
            live.days = [day: all[day], day + 1: all[day + 1]]
        case .ended(let id, let distance):
            guard let day = day(of: id) else { return }
            live.days = [:]
            commit { $0.moveDayEnd(day, to: distance) }
            syncHandles(animated: false)
        }
    }

    /// Apply a change to the draft. A change that reports false leaves no undo step.
    @discardableResult
    private func commit(undoable: Bool = true, _ change: (inout Trip) -> Bool) -> Bool {
        var draft = trip
        guard change(&draft), draft != trip else { return false }
        if undoable { undone.append(trip) }
        trip = draft
        nameUnnamedEnds()
        return true
    }

    /// Balance at once with the stops known now, then again with the stops near the new day
    /// ends once they arrive. One undo step for both. The second pass lands only while the day
    /// ends still sit where the first put them and no finger holds a handle; a place name
    /// written meanwhile does not count.
    private func balance(_ apply: @escaping (inout Trip, [PlacedStop]) -> Void) {
        settle()
        snapTask?.cancel()
        let known = handles.stops
        commit { trip in
            apply(&trip, known)
            return true
        }
        syncHandles()
        let ends = trip.dayEnds.map(\.distance)
        snapTask = Task { [weak self] in
            guard let self else { return }
            let found = await self.candidates(near: Array(ends.dropLast()))
            guard !Task.isCancelled, self.handles.activeID == nil, self.trip.dayEnds.map(\.distance) == ends
            else { return }
            self.commit(undoable: false) { trip in
                apply(&trip, found)
                return true
            }
            self.syncHandles()
        }
    }

    /// The waypoints, the stops known so far and the campsites and hotels near `distances`, all
    /// measured against the line: each answer near the point it was asked for, so a stop beside
    /// the return leg of an out-and-back lands on that leg. Offline, the known stops alone.
    private func candidates(near distances: [Double]) async -> [PlacedStop] {
        let found = (try? await finder?.stops(near: distances, on: line)) ?? []
        let stops = trip.place(trip.waypoints) + handles.stops + zip(found, distances).flatMap { answer, distance in
            trip.place(answer, near: distance)
        }
        var seen = Set<Stop>()
        let unique = stops.filter { seen.insert($0.stop).inserted }
        handles.stops = unique
        return unique
    }

    /// The handles follow the trip. A balance, an undo, an add or a remove glides; a release
    /// leaves the handle where the finger left it. A finger still on a handle lifts first, so
    /// its move is in the trip the handles are read from.
    private func syncHandles(animated: Bool = true) {
        handles.end()
        if handleIDs.count != trip.dayCount - 1 { handleIDs = (1..<max(trip.dayCount, 1)).map { _ in takeHandleID() } }
        removeBlockers = (0..<trip.dayCount).map { trip.removeDayEndBlocker($0, on: line) }
        let markers = trip.dayEnds.dropLast().enumerated().map { day, end in
            LineMarker(
                id: handleIDs[day], distance: end.distance, name: "Day \(day + 1) end",
                isFixed: removeBlockers[day] != nil)
        }
        let colors = (0..<trip.dayCount).map { OBCTheme.stageColor(index: $0) }
        withAnimation(animated ? .snappy(duration: 0.28) : nil) {
            handles.setMarkers(markers, segmentColors: colors)
        }
        stats = line.dayStats(ends: markers.map(\.distance), bikeType: trip.bikeType)
    }

    private func takeHandleID() -> Int {
        defer { nextHandleID += 1 }
        return nextHandleID
    }

    /// Name each unnamed day end after its place. A lookup never replaces a name, and an end
    /// that moved meanwhile keeps waiting for its own lookup.
    private func nameUnnamedEnds() {
        guard let placeName else { return }
        let unnamed = trip.dayEnds.enumerated()
            .filter { $0.element.name == nil }
            .map { ($0.offset, $0.element.coordinate) }
        guard !unnamed.isEmpty else { return }
        Task { [weak self] in
            for (day, coordinate) in unnamed {
                guard let name = await placeName(coordinate) else { continue }
                guard let self, self.trip.dayEnds.indices.contains(day),
                    self.trip.dayEnds[day].coordinate == coordinate, self.trip.dayEnds[day].name == nil
                else { continue }
                self.trip.namePlace(day, to: name)
            }
        }
    }
}
