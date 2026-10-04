import Foundation

extension Waypoint {
    /// Orders free-standing waypoints along the track: nearest track point, then cumulative
    /// distance, then sort and re-index in ride order. The same projection gives the signed lateral
    /// offset. The firmware converter uses the same nearest-point rule, so a file imported here and
    /// the same file dropped on the device over USB place identically. Only the name, note,
    /// coordinate and category of the input are read.
    public static func placed(_ waypoints: [Waypoint], along points: [RoutePoint]) -> [Waypoint] {
        guard !waypoints.isEmpty, points.count > 1 else { return [] }

        var cumulative: [Double] = [0]
        cumulative.reserveCapacity(points.count)
        for i in 1..<points.count {
            cumulative.append(cumulative[i - 1] + points[i - 1].coordinate.routeDistance(to: points[i].coordinate))
        }

        let placed = waypoints.map { waypoint -> (waypoint: Waypoint, along: Double, offset: Double) in
            var best = (index: 0, distance: Double.infinity)
            for (i, point) in points.enumerated() {
                let d = waypoint.coordinate.routeDistance(to: point.coordinate)
                if d < best.distance { best = (i, d) }
            }
            return (waypoint, cumulative[best.index],
                    signedOffset(of: waypoint.coordinate, at: best.index, magnitude: best.distance, along: points))
        }

        return placed
            // A non-finite `along` would break `sorted`'s strict-weak-ordering precondition and
            // trap. Import rejects such coordinates, but this keeps any other caller safe:
            // non-finite sorts to the end.
            .sorted { lhs, rhs in
                guard lhs.along.isFinite else { return false }
                guard rhs.along.isFinite else { return true }
                return lhs.along < rhs.along
            }
            .enumerated()
            .map { index, entry in
                Waypoint(
                    index: index, name: entry.waypoint.name, note: entry.waypoint.note,
                    distanceAlongMeters: entry.along, coordinate: entry.waypoint.coordinate,
                    category: entry.waypoint.category, lateralOffsetMeters: entry.offset
                )
            }
    }

    /// `magnitude` metres, signed by the side of travel the waypoint fell on: positive is right.
    /// The direction of travel at the winning point is its incoming segment; the first point has
    /// none, so it borrows its outgoing one. A waypoint on the line of travel takes the positive sign.
    static func signedOffset(of waypoint: Coordinate, at index: Int, magnitude: Double, along points: [RoutePoint]) -> Double {
        guard magnitude.isFinite else { return 0 }
        let here = points[index].coordinate
        let (from, to) = index > 0 ? (points[index - 1].coordinate, here) : (here, points[1].coordinate)
        let (dx, dy) = localMeters(from: from, to: to)
        let (ex, ey) = localMeters(from: here, to: waypoint)
        // `cross > 0` means the waypoint is left of travel, and the stored sign is
        // positive-is-right, so the stored value is the negation.
        let cross = dx * ey - dy * ex
        return cross > 0 ? -magnitude : magnitude
    }

    /// `from` to `to` as local-equirectangular metres `(east, north)`. Enough for a cross
    /// product's sign, and the projection the firmware measures with.
    private static func localMeters(from: Coordinate, to: Coordinate) -> (Double, Double) {
        let metersPerDegree = 111_320.0
        let cosLat = Foundation.cos(from.latitude * .pi / 180)
        return ((to.longitude - from.longitude) * metersPerDegree * cosLat, (to.latitude - from.latitude) * metersPerDegree)
    }
}
