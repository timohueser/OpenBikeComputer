import Foundation

extension Waypoint {
    /// Orders free-standing waypoints along the track: each projects onto its nearest segment,
    /// which gives the distance along and the signed lateral offset; then sort and re-index in ride
    /// order. A segment, not a vertex, so a sparse line (a kept line, a planned route) places a
    /// waypoint where it is. Only the name, note, coordinate and category of the input are read.
    public static func placed(_ waypoints: [Waypoint], along points: [RoutePoint]) -> [Waypoint] {
        guard !waypoints.isEmpty, points.count > 1 else { return [] }
        let line = MeasuredLine(routePoints: points)

        let placed = waypoints.map { waypoint -> (waypoint: Waypoint, along: Double, offset: Double) in
            let projection = line.projection(of: waypoint.coordinate, near: line.length / 2, window: line.length)
            let segment = min(line.index(at: projection.distance), points.count - 2)
            return (waypoint, projection.distance,
                    signedOffset(of: waypoint.coordinate, from: points[segment].coordinate, to: points[segment + 1].coordinate,
                                 magnitude: projection.error))
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

    /// `magnitude` metres, signed by the side of the segment `from` → `to` the waypoint lies on:
    /// positive is right. A waypoint on the line of travel takes the positive sign.
    static func signedOffset(of waypoint: Coordinate, from: Coordinate, to: Coordinate, magnitude: Double) -> Double {
        guard magnitude.isFinite else { return 0 }
        let (dx, dy) = localMeters(from: from, to: to)
        let (ex, ey) = localMeters(from: from, to: waypoint)
        // `cross > 0` means the waypoint is left of travel, and the stored sign is
        // positive-is-right, so the stored value is the negation.
        let cross = dx * ey - dy * ex
        return cross > 0 ? -magnitude : magnitude
    }

    /// `from` to `to` as local-equirectangular metres `(east, north)`. Enough for a cross
    /// product's sign.
    private static func localMeters(from: Coordinate, to: Coordinate) -> (Double, Double) {
        let metersPerDegree = 111_320.0
        let cosLat = Foundation.cos(from.latitude * .pi / 180)
        return ((to.longitude - from.longitude) * metersPerDegree * cosLat, (to.latitude - from.latitude) * metersPerDegree)
    }
}
