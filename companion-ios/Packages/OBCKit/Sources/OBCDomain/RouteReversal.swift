import Foundation

/// End-to-end reversal of a planned route (#503) — the pure, local transform
/// behind the detail screen's **Reverse** action. A rider who wants to ride an
/// import the other way (an out-and-back back, a loop the counter-clockwise way)
/// gets a reversed copy with one tap, no planner and no network.
///
/// It reverses the geometry and re-derives everything else the way a re-import
/// would: the point order flips, so the same `RouteStats` / `RouteObjectCodec`
/// pass that runs on any route re-derives the cumulative distance and **swaps
/// ascent and descent** for free (the climb of one direction is the descent of
/// the other). Only the waypoints need explicit work — they keep their
/// coordinates, and their `Distance Along` becomes `total_length − Distance
/// Along`, re-sorted ascending and re-indexed in the new ride order
/// (`OBCR_Spec.md` §4). No format change: the output is an ordinary
/// `ImportedRoute` that encodes to a plain OBCR file.
public extension ImportedRoute {
    /// A copy of this route flipped end to end. `name` and `creator` are carried
    /// through unchanged — the caller owns the display-name disambiguation (see
    /// ``RouteReversal/reversedName(_:)``).
    ///
    /// Degenerate inputs pass through cleanly: an empty/one-point route reverses
    /// to itself (nothing to flip), and a route with no waypoints reverses its
    /// geometry alone.
    func reversed() -> ImportedRoute {
        let reversedPoints = Array(points.reversed())
        return ImportedRoute(
            name: name,
            creator: creator,
            points: reversedPoints,
            waypoints: Self.reverseWaypoints(waypoints, totalLength: Self.length(of: points))
        )
    }

    /// Flip each waypoint's `Distance Along` about the route length, then re-sort
    /// ascending and re-index — the spec §4 rule. A waypoint at the old start
    /// (`0`) lands at the old end (`total_length`) and vice versa, so the ride
    /// order is genuinely reversed. Coordinates, notes and categories are
    /// untouched; the **lateral offset flips sign**, because "left of travel"
    /// becomes "right of travel" when you ride the line the other way (§4 stores
    /// it signed, positive = right).
    private static func reverseWaypoints(_ waypoints: [Waypoint], totalLength: Double) -> [Waypoint] {
        waypoints
            .map { waypoint -> (waypoint: Waypoint, along: Double) in
                // `total − along` clamped at 0: a waypoint projected slightly past
                // the measured length (rounding) must not sort to a negative.
                (waypoint, max(0, totalLength - waypoint.distanceAlongMeters))
            }
            // NaN-safe order, mirroring `WaypointPlacement.place` (#304): a
            // non-finite `along` would violate `sorted`'s strict-weak-ordering
            // precondition and *trap*. Import rejects the coordinates that cause
            // it upstream, but a non-import caller must not crash here either.
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
                    lateralOffsetMeters: -entry.waypoint.lateralOffsetMeters
                )
            }
    }

    /// Total polyline length in metres — the same haversine sum
    /// `RouteStats`/`RouteObjectCodec` measure distance with, so a waypoint's
    /// flipped `Distance Along` lands exactly where a re-projection would put it.
    private static func length(of points: [RoutePoint]) -> Double {
        guard points.count > 1 else { return 0 }
        var total = 0.0
        for i in 1..<points.count {
            total += points[i - 1].coordinate.distance(to: points[i].coordinate)
        }
        return total
    }
}

/// Display-name disambiguation for a reversed route — kept out of the geometry
/// transform so the naming rule is one testable place.
public enum RouteReversal {
    /// Appended to a route's name so a card makes its direction obvious.
    public static let nameSuffix = " (reversed)"

    /// `name` with the reversed suffix appended. An empty/whitespace name falls
    /// back to "Route" so the result is never a bare "(reversed)".
    public static func reversedName(_ name: String) -> String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return (trimmed.isEmpty ? "Route" : trimmed) + nameSuffix
    }
}
