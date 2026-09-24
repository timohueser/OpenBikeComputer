import Foundation

/// A gap in the line: two pieces meet without a road between them. Inside a day, the cut rides it
/// as a straight segment. At a day end, the next day starts across it; more than
/// ``Trip/transferMinMeters`` apart, that is a transfer. The router can bridge either kind, and a
/// bridged gap is line: no gap, so no transfer.
public struct TripGap: Equatable, Sendable {
    /// The line index the gap leads into.
    public let pieceStart: Int
    /// The day that rides the bridge: the day the gap is in, or the day after a day end at it.
    public let day: Int
    /// The gap is where a day ends.
    public let isAtDayEnd: Bool
    public let from: Coordinate
    public let to: Coordinate

    /// Straight metres across the gap.
    public var meters: Double { from.distance(to: to) }
}

extension Trip {
    /// Every gap of the line, in line order.
    public func gaps() -> [TripGap] {
        guard !pieceStarts.isEmpty else { return [] }
        let vertices = measuredLine.vertices
        return pieceStarts.compactMap { start in
            let at = vertices[start].distance
            guard let end = dayEnds.firstIndex(where: { $0.distance >= at - MeasuredLine.tieMeters }) else { return nil }
            let isAtDayEnd = abs(dayEnds[end].distance - at) < MeasuredLine.tieMeters && end < dayCount - 1
            return TripGap(
                pieceStart: start, day: isAtDayEnd ? end + 1 : end, isAtDayEnd: isAtDayEnd,
                from: line[start - 1].coordinate, to: line[start].coordinate)
        }
    }

    /// Route across `gap`.
    public func routeBridge(
        _ gap: TripGap, with router: any LegRouter, onDownload: @escaping @Sendable () -> Void = {}
    ) async throws -> [RoutePoint] {
        let leg = try await router.route(from: gap.from, to: gap.to, bikeType: bikeType, onDownload: onDownload)
        guard leg.count > 1 else { throw LegRouteFailure.noRoad }
        return leg
    }

    /// Join the two pieces of `gap` with `leg`, so the gap becomes line. A day end at the gap
    /// stays where it is, so the next day rides the bridge; it is no longer a transfer and loses
    /// its transfer label. False, and no change, when the gap is no longer in this line.
    @discardableResult
    public mutating func bridge(_ gap: TripGap, with leg: [RoutePoint]) -> Bool {
        guard gaps().contains(gap), let index = pieceStarts.firstIndex(of: gap.pieceStart) else { return false }
        if gap.isAtDayEnd {
            dayEnds[gap.day - 1].transfer = nil
            dayEnds[gap.day - 1].resumeName = nil
        }
        var inner = leg
        if inner.first?.coordinate == gap.from { inner.removeFirst() }
        if inner.last?.coordinate == gap.to { inner.removeLast() }
        line.insert(contentsOf: inner, at: gap.pieceStart)
        pieceStarts.remove(at: index)
        pieceStarts = pieceStarts.map { $0 > gap.pieceStart ? $0 + inner.count : $0 }
        reproject()
        return true
    }
}
