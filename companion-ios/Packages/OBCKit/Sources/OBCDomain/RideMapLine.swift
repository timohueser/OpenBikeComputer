import Foundation

/// A ride's line on the all-rides map: the tracklog simplified once at `baseToleranceMeters`.
/// A recorded segment start opens a new piece, so nothing is drawn across a gap.
public struct RideMapLine: Equatable, Sendable {
    public let id: RideID
    /// Each piece has at least two coordinates.
    public let pieces: [[Coordinate]]

    public static let baseToleranceMeters = 10.0

    public init(id: RideID, pieces: [[Coordinate]]) {
        self.id = id
        self.pieces = pieces.filter { $0.count > 1 }
    }

    public init(id: RideID, points: [RidePoint]) {
        var pieces: [[Coordinate]] = []
        var start = 0
        for index in points.indices where index == points.count - 1 || points[index + 1].segmentStart {
            pieces.append(points[start...index].map(\.coordinate))
            start = index + 1
        }
        self.init(id: id, pieces: pieces.map { Self.simplify($0, toleranceMeters: Self.baseToleranceMeters) })
    }

    public func simplified(toleranceMeters: Double) -> RideMapLine {
        RideMapLine(id: id, pieces: pieces.map { Self.simplify($0, toleranceMeters: toleranceMeters) })
    }

    /// Distance in metres from `coordinate` to the nearest point of the line.
    public func distance(to coordinate: Coordinate) -> Double {
        let plane = Plane(origin: coordinate)
        var best = Double.infinity
        for piece in pieces {
            var a = plane.point(piece[0])
            for next in piece.dropFirst() {
                let b = plane.point(next)
                best = min(best, Plane.distanceSquared(from: (0, 0), toSegment: a, b))
                a = b
            }
        }
        return best.squareRoot()
    }

    /// Douglas-Peucker: keeps the fewest vertices whose line stays within `toleranceMeters` of
    /// every dropped vertex. Always keeps both ends.
    static func simplify(_ coordinates: [Coordinate], toleranceMeters: Double) -> [Coordinate] {
        guard coordinates.count > 2 else { return coordinates }
        let plane = Plane(origin: coordinates[0])
        let points = coordinates.map(plane.point)
        let limit = toleranceMeters * toleranceMeters
        var keep = [Bool](repeating: false, count: points.count)
        keep[0] = true
        keep[points.count - 1] = true
        // An explicit stack: a long ride recurses too deep for the main thread's stack.
        var spans = [(0, points.count - 1)]
        while let (first, last) = spans.popLast() {
            guard last - first > 1 else { continue }
            var farthest = first
            var farthestDistance = 0.0
            for index in (first + 1)..<last {
                let d = Plane.distanceSquared(from: points[index], toSegment: points[first], points[last])
                if d > farthestDistance {
                    farthest = index
                    farthestDistance = d
                }
            }
            guard farthestDistance > limit else { continue }
            keep[farthest] = true
            spans.append((first, farthest))
            spans.append((farthest, last))
        }
        return coordinates.indices.filter { keep[$0] }.map { coordinates[$0] }
    }
}

/// Every ride's line at a ladder of tolerances, built once, so a zoom change only picks a level.
/// Level `k` has the tolerance `baseToleranceMeters · 4^k`.
public struct RideMapLines: Sendable {
    public static let levelCount = 6
    private let levels: [[RideMapLine]]

    public init(_ lines: [RideMapLine]) {
        var levels = [lines]
        // Each level simplifies the one below it. The errors add up to at most 4/3 of the level's
        // tolerance, which is still under one point at the zoom that picks it.
        for level in 1..<Self.levelCount {
            let tolerance = Self.tolerance(ofLevel: level)
            levels.append(levels[level - 1].map { $0.simplified(toleranceMeters: tolerance) })
        }
        self.levels = levels
    }

    private init(levels: [[RideMapLine]]) {
        self.levels = levels
    }

    public static func tolerance(ofLevel level: Int) -> Double {
        RideMapLine.baseToleranceMeters * pow(4, Double(level))
    }

    /// The lines to draw where one screen point spans `metersPerPoint`: the coarsest level whose
    /// tolerance is at most one point.
    public func lines(metersPerPoint: Double) -> [RideMapLine] {
        levels[level(metersPerPoint: metersPerPoint)]
    }

    public func restricted(to ids: Set<RideID>) -> RideMapLines {
        RideMapLines(levels: levels.map { $0.filter { ids.contains($0.id) } })
    }

    /// The ride whose drawn line passes nearest `coordinate`, if it is within `radiusMeters`.
    public func ride(
        nearest coordinate: Coordinate, withinMeters radiusMeters: Double, metersPerPoint: Double
    ) -> RideID? {
        var best: (id: RideID, distance: Double)?
        for line in lines(metersPerPoint: metersPerPoint) {
            let distance = line.distance(to: coordinate)
            if distance <= radiusMeters, distance < best?.distance ?? .infinity {
                best = (line.id, distance)
            }
        }
        return best?.id
    }

    func level(metersPerPoint: Double) -> Int {
        (1..<Self.levelCount).last { Self.tolerance(ofLevel: $0) <= metersPerPoint } ?? 0
    }
}

/// Planar metres around an origin: exact enough over one ride, or one tap radius.
private struct Plane {
    let origin: Coordinate
    let metersPerDegreeLon: Double

    init(origin: Coordinate) {
        self.origin = origin
        metersPerDegreeLon = 111_320 * cos(origin.latitude * .pi / 180)
    }

    func point(_ c: Coordinate) -> (x: Double, y: Double) {
        ((c.longitude - origin.longitude) * metersPerDegreeLon, (c.latitude - origin.latitude) * 111_320)
    }

    static func distanceSquared(
        from p: (x: Double, y: Double), toSegment a: (x: Double, y: Double), _ b: (x: Double, y: Double)
    ) -> Double {
        let ab = (x: b.x - a.x, y: b.y - a.y)
        let lengthSquared = ab.x * ab.x + ab.y * ab.y
        let t = lengthSquared > 0
            ? min(max(((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / lengthSquared, 0), 1)
            : 0
        let dx = a.x + t * ab.x - p.x, dy = a.y + t * ab.y - p.y
        return dx * dx + dy * dy
    }
}
