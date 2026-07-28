import Foundation
import OBCDomain

/// A waypoint as a route file carries it — named and positioned, but with no
/// ride-order context yet. Each decoder collects these, then `WaypointPlacement`
/// turns them into the ordered `Waypoint`s W1 renders.
struct RawWaypoint {
    var name: String
    var note: String?
    let coordinate: Coordinate
    /// The icon the exporter wrote — GPX `<sym>`/`<type>`, TCX `PointType` — kept
    /// verbatim; ``WaypointSymbol`` maps it onto a category during placement.
    var symbol: String = ""
}

/// Shared by every `RouteFileDecoder`: order free-standing waypoints along the
/// track via nearest-track-point projection → cumulative distance, then sort +
/// re-index in ride order. (GPX carries waypoints file-level and unordered; TCX
/// course points are usually ordered already — projecting both keeps
/// `distanceAlongMeters` consistent across formats.)
///
/// The same projection fixes the waypoint's **signed lateral offset**: its
/// magnitude is the distance to the track point that won the placement, its sign
/// which side of the direction of travel it fell on (`OBCR_Spec.md` §4). The
/// firmware's converter derives both the same way from the same nearest-point
/// rule, so a GPX imported here and the same file dropped on the device over USB
/// place — and categorize — identically.
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
            return Placement(
                waypoint: waypoint,
                along: cumulative[best.index],
                lateralOffset: signedOffset(
                    of: waypoint.coordinate, at: best.index, magnitude: best.distance, along: points
                )
            )
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
                    coordinate: placement.waypoint.coordinate,
                    category: WaypointSymbol.category(for: placement.waypoint.symbol),
                    lateralOffsetMeters: placement.lateralOffset
                )
            }
    }

    /// `magnitude` metres, signed by the side of travel the waypoint fell on:
    /// **positive = right**, negative = left, matching `OBCR_Spec.md` §4.
    ///
    /// The direction of travel at the winning point `index` is its **incoming**
    /// segment; the first point has none, so it borrows its outgoing one (the
    /// firmware converter resolves that case the same way, one point later in its
    /// single streaming pass). A waypoint exactly on the line of travel — cross
    /// product zero, including a degenerate repeated point — takes the positive
    /// sign; at the magnitudes where a side is drawn at all, that does not occur
    /// in practice.
    private static func signedOffset(
        of waypoint: Coordinate, at index: Int, magnitude: Double, along points: [RoutePoint]
    ) -> Double {
        guard magnitude.isFinite else { return 0 }
        let here = points[index].coordinate
        let (from, to) = index > 0
            ? (points[index - 1].coordinate, here)
            : (here, points[1].coordinate)
        let (dx, dy) = localMeters(from: from, to: to)
        let (ex, ey) = localMeters(from: here, to: waypoint)
        // `cross > 0` ⇒ the waypoint lies left of travel; the stored sign is
        // positive-is-right, so the stored value is its negation.
        let cross = dx * ey - dy * ex
        return cross > 0 ? -magnitude : magnitude
    }

    /// `from → to` as local-equirectangular metres `(east, north)` around `from`'s
    /// latitude — enough for a cross product's sign over a route's short segments,
    /// and the same projection the firmware measures with.
    private static func localMeters(from: Coordinate, to: Coordinate) -> (Double, Double) {
        let metersPerDegree = 111_320.0
        let cosLat = Foundation.cos(from.latitude * .pi / 180)
        return (
            (to.longitude - from.longitude) * metersPerDegree * cosLat,
            (to.latitude - from.latitude) * metersPerDegree
        )
    }

    private struct Placement {
        let waypoint: RawWaypoint
        let along: Double
        let lateralOffset: Double
    }
}
