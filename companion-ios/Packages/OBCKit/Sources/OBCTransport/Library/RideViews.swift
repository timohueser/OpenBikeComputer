import Foundation
import OBCDomain

/// Rides as the rider sees them: each edited ride, and each synced ride that no edit covers. An
/// edit writes only the list of views; the synced rides stay as they were synced, so sync keeps
/// recognising them and Revert restores them exactly.
extension LibraryStore {
    /// Every ride the rider sees, newest first.
    public func rideSummaries() -> [RideSummary] {
        let views = rideViews()
        let covered = Set(views.flatMap(\.sources))
        return (archivedRideSummaries().filter { !covered.contains($0.id) } + views.map(\.summary))
            .sorted { $0.date > $1.date }
    }

    /// One ride's full tracklog, loaded on demand for the detail. Nil when the ride is unknown
    /// or its points do not decode.
    public func ridePoints(_ id: RideID) -> [RidePoint]? {
        guard let view = rideViews().first(where: { $0.id == id }) else { return archivedRidePoints(id) }
        return RideEdit.points(of: view.slices, source: archivedRidePoints)
    }

    public func isEditedRide(_ id: RideID) -> Bool {
        rideViews().contains { $0.id == id }
    }

    /// The rename and bike-type write path.
    public func saveRideSummary(_ summary: RideSummary) {
        var views = rideViews()
        guard let index = views.firstIndex(where: { $0.id == summary.id }) else {
            return saveArchivedRideSummary(summary)
        }
        views[index].summary = summary
        saveRideViews(views)
    }

    /// Delete a ride's files. An edited ride deletes the synced rides that no other edit still
    /// covers, and marks them deleted, so a sync does not bring them back.
    public func deleteRide(_ id: RideID) {
        var views = rideViews()
        guard let index = views.firstIndex(where: { $0.id == id }) else { return deleteArchivedRide(id) }
        let view = views.remove(at: index)
        saveRideViews(views)
        let covered = Set(views.flatMap(\.sources))
        for source in view.sources.subtracting(covered) {
            deleteArchivedRide(source)
            markRideDeleted(source)
        }
    }

    // MARK: Edits

    /// Keep only the part of the ride inside `range`. False when fewer than two points remain.
    @discardableResult
    public func trimRide(_ id: RideID, to range: ClosedRange<Date>, summary: RideSummary) -> Bool {
        var edit = RideEditSession(store: self)
        guard let slices = edit.slices(of: id),
              let view = edit.view(id: id, summary: summary, slices: RideEdit.trimmed(slices, to: range))
        else { return false }
        edit.replace([id], with: [view])
        return true
    }

    /// Make two rides of one at `time`: the first keeps the id, and the names get "(1)" and
    /// "(2)". Returns the second ride's id, or nil when a part would have fewer than two points.
    @discardableResult
    public func splitRide(_ id: RideID, at time: Date, summary: RideSummary) -> RideID? {
        var edit = RideEditSession(store: self)
        guard let slices = edit.slices(of: id) else { return nil }
        let parts = RideEdit.split(slices, at: time)
        var first = summary, second = summary
        first.name = "\(summary.name) (1)"
        second.name = "\(summary.name) (2)"
        let secondID = RideID("edit-\(UUID().uuidString.lowercased())")
        guard let before = edit.view(id: id, summary: first, slices: parts.before),
              let after = edit.view(id: secondID, summary: second, slices: parts.after)
        else { return nil }
        edit.replace([id], with: [before, after])
        return secondID
    }

    /// Join `second` onto the end of `first`. The joined ride keeps the first ride's id, name and
    /// bike type.
    @discardableResult
    public func mergeRides(_ first: RideSummary, _ second: RideID) -> Bool {
        var edit = RideEditSession(store: self)
        guard let head = edit.slices(of: first.id), let tail = edit.slices(of: second),
              let view = edit.view(id: first.id, summary: first, slices: RideEdit.joined(head, tail))
        else { return false }
        edit.replace([first.id, second], with: [view])
        return true
    }

    /// Remove the edits that share a synced ride with `id`, so each of those synced rides shows
    /// again as it was synced.
    public func revertRide(_ id: RideID) {
        saveRideViews(RideEdit.reverted(rideViews(), id: id))
    }
}

/// One edit's reads: each synced tracklog decodes once.
private struct RideEditSession<Store: LibraryStore> {
    let store: Store
    var views: [RideView]
    private var points: [RideID: [RidePoint]] = [:]

    init(store: Store) {
        self.store = store
        views = store.rideViews()
    }

    /// The view's slices, or the whole synced ride for an unedited one.
    mutating func slices(of id: RideID) -> [RideSlice]? {
        if let view = views.first(where: { $0.id == id }) { return view.slices }
        guard let all = source(id), let first = all.first, let last = all.last else { return nil }
        return [RideSlice(source: id, start: first.timestamp, end: last.timestamp)]
    }

    mutating func view(id: RideID, summary: RideSummary, slices: [RideSlice]) -> RideView? {
        guard let points = RideEdit.points(of: slices, source: { source($0) }), points.count > 1
        else { return nil }
        return RideView(summary: summary.withStats(of: points, id: id), slices: slices)
    }

    func replace(_ ids: [RideID], with new: [RideView]) {
        store.saveRideViews(views.filter { !ids.contains($0.id) } + new)
    }

    private mutating func source(_ id: RideID) -> [RidePoint]? {
        if let cached = points[id] { return cached }
        let loaded = store.archivedRidePoints(id)
        points[id] = loaded
        return loaded
    }
}
