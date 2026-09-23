import Foundation

/// A gap inside a day: two pieces of the line meet away from every day end. The cut rides it as
/// a straight segment until the router bridges it.
public struct TripGap: Equatable, Sendable {
    /// The line index the gap leads into.
    public let pieceStart: Int
    public let day: Int
    public let from: Coordinate
    public let to: Coordinate

    /// Straight metres across the gap.
    public var meters: Double { from.distance(to: to) }
}

extension Trip {
    /// The gaps inside days, in line order. A gap at a day end, a transfer or not, is where the
    /// next day starts: it stays, and the device's "Ride to start" covers it.
    public func gapsInsideDays() -> [TripGap] {
        guard !pieceStarts.isEmpty else { return [] }
        let vertices = measuredLine.vertices
        return pieceStarts.compactMap { start in
            let at = vertices[start].distance
            guard let day = dayEnds.firstIndex(where: { $0.distance >= at - MeasuredLine.tieMeters }),
                abs(dayEnds[day].distance - at) >= MeasuredLine.tieMeters
            else { return nil }
            return TripGap(pieceStart: start, day: day, from: line[start - 1].coordinate, to: line[start].coordinate)
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

    /// Join the two pieces of `gap` with `leg`, so the gap becomes line. False, and no change,
    /// when the gap is no longer inside a day of this line.
    @discardableResult
    public mutating func bridge(_ gap: TripGap, with leg: [RoutePoint]) -> Bool {
        guard gapsInsideDays().contains(gap), let index = pieceStarts.firstIndex(of: gap.pieceStart) else { return false }
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
