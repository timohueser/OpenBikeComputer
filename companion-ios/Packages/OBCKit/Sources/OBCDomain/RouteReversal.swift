import Foundation

/// End-to-end reversal of a planned route: a pure, local transform. It flips the point order
/// and re-derives everything else the way a re-import would, so the usual `RouteStats` pass
/// swaps ascent and descent for free. Only the waypoints need explicit work: `Distance Along`
/// becomes `total_length - Distance Along` (`OBCR_Spec.md`).
public extension ImportedRoute {
    /// A copy of this route flipped end to end. `name` and `creator` are unchanged: the caller
    /// owns the display-name disambiguation (see ``RouteReversal/reversedName(_:)``).
    func reversed() -> ImportedRoute {
        let reversedPoints = points.indices.reversed().map { i in
            RoutePoint(coordinate: points[i].coordinate, elevationMeters: points[i].elevationMeters, surface: i + 1 < points.count ? points[i+1].surface : 0, elevationIncomplete: i + 1 < points.count ? points[i+1].elevationIncomplete : false)
        }
        return ImportedRoute(
            name: name,
            creator: creator,
            points: reversedPoints,
            waypoints: Self.reverseWaypoints(waypoints, totalLength: Self.length(of: points))
        )
    }

    /// Flip each waypoint's `Distance Along` about the route length, then re-sort ascending and
    /// re-index. The lateral offset flips sign: "left of travel" becomes "right of travel" when
    /// you ride the line the other way (`OBCR_Spec.md` stores it signed, positive = right).
    private static func reverseWaypoints(_ waypoints: [Waypoint], totalLength: Double) -> [Waypoint] {
        waypoints
            .map { waypoint -> (waypoint: Waypoint, along: Double) in
                // `total - along` clamped at 0: rounding can project a waypoint slightly past
                // the measured length, and it must not sort as a negative.
                (waypoint, max(0, totalLength - waypoint.distanceAlongMeters))
            }
            // NaN-safe order: a non-finite `along` would violate `sorted`'s strict-weak-ordering
            // precondition and trap. Import rejects the coordinates that cause it upstream, but a
            // non-import caller must not crash here either.
            .sorted { lhs, rhs in
                guard lhs.along.isFinite else { return false }
                guard rhs.along.isFinite else { return true }
                return lhs.along < rhs.along
            }
            .enumerated()
            .map { index, entry in
                Waypoint(
                    index: index,
                    name: entry.waypoint.name,
                    note: entry.waypoint.note,
                    distanceAlongMeters: entry.along,
                    coordinate: entry.waypoint.coordinate,
                    category: entry.waypoint.category,
                    lateralOffsetMeters: -entry.waypoint.lateralOffsetMeters,
                    provenance: entry.waypoint.provenance
                )
            }
    }

    /// Total polyline length in metres: the same haversine sum `RouteStats` measures distance
    /// with, so a flipped `Distance Along` lands where a re-projection would put it.
    private static func length(of points: [RoutePoint]) -> Double {
        guard points.count > 1 else { return 0 }
        var total = 0.0
        for i in 1..<points.count {
            total += points[i - 1].coordinate.routeDistance(to: points[i].coordinate)
        }
        return total
    }
}

/// Display-name disambiguation for a reversed route.
public enum RouteReversal {
    public static let nameSuffix = " (reversed)"

    public static func reversedName(_ name: String) -> String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return (trimmed.isEmpty ? "Route" : trimmed) + nameSuffix
    }
}
