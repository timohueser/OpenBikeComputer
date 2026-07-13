import Foundation
import OBCDomain

/// A waypoint as a route file carries it — named and positioned, but with no
/// ride-order context yet. Each decoder collects these, then `WaypointPlacement`
/// turns them into the ordered `Waypoint`s W1 renders.
struct RawWaypoint {
    var name: String
    var note: String?
    let coordinate: Coordinate
}

/// Shared by every `RouteFileDecoder`: order free-standing waypoints along the
/// track via nearest-track-point projection → cumulative distance, then sort +
/// re-index in ride order. (GPX carries waypoints file-level and unordered; TCX
/// course points are usually ordered already — projecting both keeps
/// `distanceAlongMeters` consistent across formats.)
enum WaypointPlacement {
    static func place(_ raw: [RawWaypoint], along points: [RoutePoint]) -> [Waypoint] {
        guard !raw.isEmpty, points.count > 1 else { return [] }

        var cumulative: [Double] = [0]
        cumulative.reserveCapacity(points.count)
        for i in 1..<points.count {
            cumulative.append(cumulative[i - 1] + points[i - 1].coordinate.distance(to: points[i].coordinate))
        }

        let placed = raw.map { waypoint -> Placement in
            var best = (index: 0, distance: Double.infinity)
            for (i, point) in points.enumerated() {
                let d = waypoint.coordinate.distance(to: point.coordinate)
                if d < best.distance { best = (i, d) }
            }
            return Placement(waypoint: waypoint, along: cumulative[best.index])
        }

        return placed
            // NaN-safe order (#304): a non-finite `along` (from a non-finite
            // route coordinate poisoning the cumulative distance) would violate
            // `sorted`'s strict-weak-ordering precondition and *trap*. Import
            // rejects such coordinates upstream now, but this keeps any
            // non-import caller from crashing — non-finite sorts to the end.
            .sorted { lhs, rhs in
                guard lhs.along.isFinite else { return false }
                guard rhs.along.isFinite else { return true }
                return lhs.along < rhs.along
            }
            .enumerated()
            .map { index, placement in
                Waypoint(
                    index: index,
                    name: placement.waypoint.name,
                    note: placement.waypoint.note,
                    distanceAlongMeters: placement.along,
                    coordinate: placement.waypoint.coordinate
                )
            }
    }

    private struct Placement {
        let waypoint: RawWaypoint
        let along: Double
    }
}
