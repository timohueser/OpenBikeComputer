import Foundation

/// A polyline measured along its length: every vertex carries its cumulative distance,
/// elevation and cumulative climb, so a position on the line is one number, a distance in
/// metres. The marker-on-line control, the day editor and ride trims all speak in these
/// distances.
///
/// A line is made of pieces. The segment into a piece start is a gap: it carries no distance,
/// nothing projects onto it and nothing is drawn across it.
public struct MeasuredLine: Equatable, Sendable {
    public struct Vertex: Equatable, Sendable {
        public let coordinate: Coordinate
        /// Metres along the line from the start, with gaps counting nothing.
        public let distance: Double
        /// Elevation in metres. A missing source value repeats the last known one.
        public let elevation: Double
        /// Cumulative climb in metres, with the same hysteresis as `RouteStats`.
        public let climb: Double
        /// Cumulative descent in metres, with the same hysteresis.
        public let descent: Double
    }

    public let vertices: [Vertex]
    /// Vertex indices that start a new piece. Never contains 0.
    public let pieceStarts: Set<Int>
    /// Whether any source point carried an elevation. Without one, every vertex reads 0 m.
    public let hasElevation: Bool

    /// The line's length in metres.
    public var length: Double { vertices.last?.distance ?? 0 }

    /// `elevations` is index-aligned with `coordinates`; a short or empty array reads as
    /// missing. `pieceStarts` lists the vertices a gap leads into. `distanceBetween` changes
    /// the metric without changing the vertex and gap rules.
    public init(
        coordinates: [Coordinate], elevations: [Double?] = [], pieceStarts: [Int] = [],
        distanceBetween: (Coordinate, Coordinate) -> Double = { $0.routeDistance(to: $1) }
    ) {
        let starts = Set(pieceStarts.filter { $0 > 0 && $0 < coordinates.count })
        var vertices: [Vertex] = []
        vertices.reserveCapacity(coordinates.count)
        var distance = 0.0
        var climb = 0.0
        var descent = 0.0
        var confirmed: Double?
        var elevation = elevations.lazy.compactMap { $0 }.first ?? 0
        for (index, coordinate) in coordinates.enumerated() {
            if index < elevations.count, let known = elevations[index] { elevation = known }
            if index > 0, !starts.contains(index) {
                distance += distanceBetween(coordinates[index - 1], coordinate)
            }
            if let last = confirmed {
                if elevation >= last + RouteStats.climbHysteresisMeters {
                    climb += elevation - last
                    confirmed = elevation
                } else if elevation <= last - RouteStats.climbHysteresisMeters {
                    descent += last - elevation
                    confirmed = elevation
                }
            } else {
                confirmed = elevation
            }
            vertices.append(Vertex(
                coordinate: coordinate, distance: distance, elevation: elevation, climb: climb, descent: descent
            ))
        }
        self.vertices = vertices
        self.pieceStarts = starts
        hasElevation = elevations.contains { $0 != nil }
    }

    public init(routePoints: [RoutePoint]) {
        self.init(
            coordinates: routePoints.map(\.coordinate),
            elevations: routePoints.map(\.elevationMeters)
        )
    }

    /// A recorded track: each segment start opens a new piece.
    public init(ridePoints: [RidePoint]) {
        self.init(
            coordinates: ridePoints.map(\.coordinate),
            elevations: ridePoints.map(\.elevationMeters),
            pieceStarts: ridePoints.indices.filter { $0 > 0 && ridePoints[$0].segmentStart }
        )
    }

    // MARK: Lookup by distance

    /// The index of the last vertex at or before `distance`, so the position lies on the
    /// segment that starts there. A distance shared by a piece end and the next piece start
    /// resolves to the piece end.
    public func index(at distance: Double) -> Int {
        guard vertices.count > 1 else { return 0 }
        var low = 0
        var high = vertices.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if vertices[mid].distance <= distance { low = mid } else { high = mid - 1 }
        }
        // Exactly on a piece boundary, the position belongs to the piece end.
        while low > 0, vertices[low].distance == distance, pieceStarts.contains(low) {
            low -= 1
        }
        return low
    }

    public func coordinate(at distance: Double) -> Coordinate {
        interpolate(at: distance) { a, b, t in
            Coordinate(
                latitude: a.coordinate.latitude + (b.coordinate.latitude - a.coordinate.latitude) * t,
                longitude: a.coordinate.longitude + (b.coordinate.longitude - a.coordinate.longitude) * t
            )
        }
    }

    public func elevation(at distance: Double) -> Double {
        interpolate(at: distance) { a, b, t in a.elevation + (b.elevation - a.elevation) * t }
    }

    /// `count` elevations at even distances from the start to the end: a profile on a true
    /// distance axis. Empty when the line has no elevation.
    public func elevationProfile(count: Int) -> [Double] {
        guard hasElevation, count > 1, vertices.count > 1 else { return [] }
        return (0..<count).map { elevation(at: length * Double($0) / Double(count - 1)) }
    }

    /// A ride's profile card: `RouteStats.profileSampleCount` samples on a distance axis.
    public static func elevationProfile(ridePoints: [RidePoint]) -> [Double] {
        MeasuredLine(ridePoints: ridePoints).elevationProfile(count: RouteStats.profileSampleCount)
    }

    /// Climb between two distances, from the cumulative walk: O(log n), so it can run on
    /// every drag frame.
    public func climb(from: Double, to: Double) -> Double {
        max(0, cumulative(\.climb, at: to) - cumulative(\.climb, at: from))
    }

    public func descent(from: Double, to: Double) -> Double {
        max(0, cumulative(\.descent, at: to) - cumulative(\.descent, at: from))
    }

    private func cumulative(_ key: KeyPath<Vertex, Double>, at distance: Double) -> Double {
        interpolate(at: distance) { a, b, t in a[keyPath: key] + (b[keyPath: key] - a[keyPath: key]) * t }
    }

    private func interpolate<T>(at distance: Double, _ mix: (Vertex, Vertex, Double) -> T) -> T {
        precondition(!vertices.isEmpty, "an empty line has no positions")
        let clamped = min(max(distance, 0), length)
        let i = index(at: clamped)
        guard i + 1 < vertices.count else { return mix(vertices[i], vertices[i], 0) }
        let a = vertices[i], b = vertices[i + 1]
        let span = b.distance - a.distance
        let t = span > 0 ? (clamped - a.distance) / span : 0
        return mix(a, b, t)
    }

    // MARK: Projection

    /// The distance along the line of the point nearest `point`, searched only within
    /// `window` metres of `near`. The window is what keeps a dragged marker on its own part
    /// of an out-and-back or a switchback: the finger over the other leg is not "near".
    /// Gap segments are skipped, so a result never lies inside a gap.
    public func project(_ point: Coordinate, near: Double, window: Double) -> Double {
        projection(of: point, near: near, window: window).distance
    }

    /// `project` with how far the point is from the line at that distance, in metres. Two
    /// candidates within `tieMeters` of each other count as a tie, and the one nearer `near`
    /// wins it: a finger on the lane between the legs of an out-and-back stays on its leg.
    public func projection(
        of point: Coordinate, near: Double, window: Double
    ) -> (distance: Double, error: Double) {
        guard vertices.count > 1 else { return (0, 0) }
        let first = index(at: max(near - window, 0))
        let last = min(index(at: min(near + window, length)) + 1, vertices.count - 1)
        // Planar metres around the search centre; the window is small next to the Earth.
        let origin = vertices[first].coordinate
        let metersPerDegreeLon = 111_320 * cos(origin.latitude * .pi / 180)
        func planar(_ c: Coordinate) -> (x: Double, y: Double) {
            ((c.longitude - origin.longitude) * metersPerDegreeLon, (c.latitude - origin.latitude) * 111_320)
        }
        let p = planar(point)
        var best = (distance: near, error: Double.infinity)
        let tie = Self.tieMeters * Self.tieMeters
        for i in first..<last where !pieceStarts.contains(i + 1) {
            let a = planar(vertices[i].coordinate), b = planar(vertices[i + 1].coordinate)
            let ab = (x: b.x - a.x, y: b.y - a.y)
            let lengthSquared = ab.x * ab.x + ab.y * ab.y
            let t = lengthSquared > 0
                ? min(max(((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / lengthSquared, 0), 1)
                : 0
            let dx = a.x + t * ab.x - p.x, dy = a.y + t * ab.y - p.y
            let error = dx * dx + dy * dy
            let segment = vertices[i + 1].distance - vertices[i].distance
            let candidate = vertices[i].distance + t * segment
            let closer = error < best.error - tie
            let tied = abs(error - best.error) <= tie && abs(candidate - near) < abs(best.distance - near)
            if closer || tied {
                best = (candidate, error)
            }
        }
        return (min(max(best.distance, near - window), near + window), best.error.squareRoot())
    }

    /// Projection candidates closer to each other than this are one place.
    public static let tieMeters = 1.0
}
