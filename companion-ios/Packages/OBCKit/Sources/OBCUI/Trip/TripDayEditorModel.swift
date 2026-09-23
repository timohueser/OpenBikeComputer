import SwiftUI
import Observation
import OBCDomain

/// The state under the day editor: a draft of the trip, the handle control that moves its day
/// ends, the live day figures, and one undo step per change. Views drive it and Done hands the
/// draft back through `onSave`; nothing is saved before.
///
/// A drag is live in `stats` on every frame and reaches the trip once, when the finger lets
/// go. Balancing runs twice per tap: at once with the stops already known, then again when
/// Apple Maps answers for the new day ends, so the stepper responds under the finger and the
/// snap follows a moment later.
@MainActor @Observable
public final class TripDayEditorModel {
    public private(set) var trip: Trip
    /// The day-end handles on the map and the profile. Colours follow the day index.
    public let handles: LineMarkerEditorModel
    /// The figures of every day for the handle positions of this frame.
    public private(set) var stats: [DayStats] = []
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
    @ObservationIgnored private var snapTask: Task<Void, Never>?
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
    public func removeBlocker(_ day: Int) -> String? { trip.removeDayEndBlocker(day) }

    // MARK: Changes

    /// Split mode: cut the line into `count` days.
    public func setDayCount(_ count: Int) {
        let days = min(max(count, 1), Trip.maxSplitDays)
        balance { trip, candidates in trip.split(into: days, candidates: candidates) }
    }

    /// Re-balance every day by riding time, keeping the number of days.
    public func evenOut() {
        balance { trip, candidates in trip.evenOut(candidates: candidates) }
    }

    /// A new day end in the middle of the longest day. Returns the new day's index.
    @discardableResult
    public func addDayEnd() -> Int? {
        var added: Int?
        commit { trip in
            added = trip.addDayEnd()
            return added != nil
        }
        if let added { handleIDs.insert(takeHandleID(), at: added) }
        syncHandles()
        return added
    }

    /// Remove `day`'s end, so the day joins the next one.
    public func removeDayEnd(_ day: Int) {
        guard commit({ $0.removeDayEnd(day) }) else { return }
        handleIDs.remove(at: day)
        syncHandles()
    }

    public func renameDay(_ day: Int, to title: String?) {
        commit { trip in
            trip.renameDay(day, to: title)
            return true
        }
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
        guard let previous = undone.popLast() else { return }
        snapTask?.cancel()
        trip = previous
        syncHandles()
    }

    public func save() {
        snapTask?.cancel()
        onSave(trip)
    }

    // MARK: Mechanics

    private func handle(_ event: LineMarkerEvent) {
        switch event {
        case .began:
            break
        case .moved:
            stats = line.dayStats(ends: handles.markers.map(\.distance), bikeType: trip.bikeType)
        case .ended(let id, let distance):
            guard let day = handleIDs.firstIndex(of: id) else { return }
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
    /// ends once they arrive. One undo step for both.
    private func balance(_ apply: @escaping (inout Trip, [PlacedStop]) -> Void) {
        snapTask?.cancel()
        let known = handles.stops
        commit { trip in
            apply(&trip, known)
            return true
        }
        syncHandles()
        let ends = trip.dayEnds.dropLast().map(\.distance)
        let before = trip
        snapTask = Task { [weak self] in
            guard let self else { return }
            let found = await self.candidates(near: ends)
            guard !Task.isCancelled, self.trip == before else { return }
            self.commit(undoable: false) { trip in
                apply(&trip, found)
                return true
            }
            self.syncHandles()
        }
    }

    /// The waypoints and the campsites and hotels near `distances`, all measured against the
    /// line. Offline, the waypoints alone.
    private func candidates(near distances: [Double]) async -> [PlacedStop] {
        let found = (try? await finder?.stops(near: distances, on: line)) ?? []
        let stops = trip.place(trip.waypoints + found.flatMap { $0 })
        var seen = Set<Stop>()
        let unique = stops.filter { seen.insert($0.stop).inserted }
        handles.stops = unique
        return unique
    }

    /// The handles follow the trip. A balance, an undo, an add or a remove glides; a release
    /// leaves the handle where the finger left it.
    private func syncHandles(animated: Bool = true) {
        if handleIDs.count != trip.dayCount - 1 { handleIDs = (1..<max(trip.dayCount, 1)).map { _ in takeHandleID() } }
        let markers = trip.dayEnds.dropLast().enumerated().map { day, end in
            LineMarker(id: handleIDs[day], distance: end.distance, name: "Day \(day + 1) end", isFixed: trip.endsAtTransfer(day))
        }
        let colors = (0..<trip.dayCount).map { OBCTheme.stageColor(index: $0) }
        withAnimation(animated ? .snappy(duration: 0.28) : nil) {
            handles.setMarkers(markers, segmentColors: colors)
        }
        stats = line.dayStats(ends: markers.map(\.distance), bikeType: trip.bikeType)
    }

    /// The day the handle belongs to.
    public func day(of handle: LineMarker.ID) -> Int? { handleIDs.firstIndex(of: handle) }

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
