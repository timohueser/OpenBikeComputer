import Foundation
import OBCDomain

/// One record of a catalog cell file, as `specs/route-catalog.md` specifies: a route record or a long route.
public struct CatalogRecord: Decodable, Equatable, Sendable {
    public enum Kind: String, Decodable, Sendable { case hiking, foot, bicycle, mtb }
    public let id: Int
    public let kind: Kind
    public let name: String?
    public let ref: String?
    public let `operator`: String?
    public let description: String?
    public let website: String?
    public let symbol: String?
    public let rank: Int
    public let loop: Bool
    public let length_m: Double
    public let ascent_m: Double
    public let descent_m: Double
    public let grades_m: [Double]?
    public let hardest: Int?
    public let cells: [String]
    public let line_udeg: [Int]?
    public let via: [Int]?
    public let turnarounds: [Int]?
    public let parent: Int?
    public let stage: Int?
    public let stages: [Int]?
    public let start_udeg: [Int]?

    /// The decoded line of a route record; empty for a long route or a line with an unpaired value.
    public var line: [Coordinate] { decodeCoordinates(line_udeg ?? []) ?? [] }

    /// The start, the shaping points and the finish, and the plan indices where the route turns back on purpose.
    /// Nil for a long route, and for a record whose shaping points or turnarounds do not fit its line.
    public var plan: (points: [Coordinate], turnarounds: [Int])? {
        let line = line, turnarounds = turnarounds ?? []
        guard let via, line.count > 1, via.allSatisfy({ (1..<line.count - 1).contains($0) }),
              let first = line.first, let last = line.last else { return nil }
        // A loop can turn back at its start, vertex 0, which is plan index 0.
        let indices = turnarounds.compactMap { $0 == 0 && loop ? 0 : via.firstIndex(of: $0).map { $0 + 1 } }
        guard indices.count == turnarounds.count else { return nil }
        return ([first] + via.map { line[$0] } + [last], indices)
    }
}

/// The activities of the route search; each lists its own route kinds.
public enum RouteActivity: String, CaseIterable, Sendable {
    case hiking, mtb, road, gravel, touring
    var kinds: Set<CatalogRecord.Kind> {
        switch self { case .hiking: [.hiking, .foot]; case .mtb: [.mtb]; case .road, .gravel, .touring: [.bicycle] }
    }
}

public enum RouteShape: String, Sendable { case any, loop, oneWay = "one-way" }
/// Inclusive bounds. A nil bound is no limit.
public struct RouteBounds: Equatable, Sendable {
    public var from: Double?
    public var to: Double?
    public init(from: Double? = nil, to: Double? = nil) { self.from = from; self.to = to }
    func contains(_ value: Double, scale: Double = 1) -> Bool {
        (from ?? -.infinity) * scale <= value && value <= (to ?? .infinity) * scale
    }
}
public enum RouteSort: String, Sendable { case nearest, shortest, longest, mostClimb = "most-climb", leastClimb = "least-climb" }

public struct RouteQuery: Equatable, Sendable {
    public var start: Coordinate
    public var radiusKm: Double
    public var activity: RouteActivity
    public var shape: RouteShape
    public var distanceKm: RouteBounds
    public var climbM: RouteBounds
    /// Grade indices of the hardest part, 0 to 3: T1–T4 for hiking, S0–S3 for mountain bike. Other activities ignore it.
    public var hardest: ClosedRange<Int>?
    public var sort: RouteSort
    /// Offline: the downloaded cells. Only routes that lie wholly inside them match.
    public var covered: Set<String>?

    public init(start: Coordinate, radiusKm: Double, activity: RouteActivity, shape: RouteShape = .loop,
                distanceKm: RouteBounds = .init(), climbM: RouteBounds = .init(),
                hardest: ClosedRange<Int>? = nil, sort: RouteSort = .nearest, covered: Set<String>? = nil) {
        self.start = start; self.radiusKm = radiusKm; self.activity = activity; self.shape = shape
        self.distanceKm = distanceKm; self.climbM = climbM; self.hardest = hardest; self.sort = sort; self.covered = covered
    }
}

public struct RouteMatch: Equatable, Sendable {
    public let route: CatalogRecord
    public let distanceM: Double
}

/// The records of one cell, or nil when the cell is not covered.
public typealias RouteCellLoader = @Sendable (String) async throws -> [CatalogRecord]?

public enum SignedRoutes {
    private static let kmPerDegree = 6371 * Double.pi / 180

    /// The routes within the radius that pass the filters, in the sort order with ties by id. The distance is from the
    /// start to the nearest point of the route; a long route measures to its own start. A search at a larger radius
    /// holds every match of a smaller one.
    public static func search(_ query: RouteQuery, loadCell: @escaping RouteCellLoader) async throws -> [RouteMatch] {
        let cells = cells(around: query.start, radiusKm: query.radiusKm).filter { query.covered?.contains($0) ?? true }
        let records = try await withThrowingTaskGroup(of: [CatalogRecord]?.self) { group in
            for cell in cells { group.addTask { try await loadCell(cell) } }
            var records: [Int: CatalogRecord] = [:]
            for try await file in group { file?.forEach { records[$0.id] = $0 } }
            return records
        }
        let matches = records.values.filter { passes($0, query) }.compactMap { route in
            let line = route.start_udeg.map { decodeCoordinates($0) ?? [] } ?? route.line
            return distanceKm(from: query.start, to: line, within: query.radiusKm).map { RouteMatch(route: route, distanceM: $0 * 1000) }
        }
        let key: (RouteMatch) -> Double = switch query.sort {
        case .nearest: { $0.distanceM }
        case .shortest: { $0.route.length_m }
        case .longest: { -$0.route.length_m }
        case .mostClimb: { -$0.route.ascent_m }
        case .leastClimb: { $0.route.ascent_m }
        }
        return matches.sorted { (key($0), $0.route.id) < (key($1), $1.route.id) }
    }

    /// Index of the line vertex nearest to `place`, the first on a tie: where a loop plan starts. Nil for an empty line.
    public static func nearestVertex(_ line: [Coordinate], to place: Coordinate) -> Int? {
        guard !line.isEmpty else { return nil }
        let kx = cos(place.latitude * .pi / 180)
        func squared(_ vertex: Coordinate) -> Double {
            let dx = (vertex.longitude - place.longitude) * kx, dy = vertex.latitude - place.latitude
            return dx * dx + dy * dy
        }
        return line.indices.reduce(0) { best, index in squared(line[index]) < squared(line[best]) ? index : best }
    }

    /// The zoom 9 cells `9-X-Y` whose tiles meet the box of the start ± the radius.
    static func cells(around start: Coordinate, radiusKm: Double) -> [String] {
        let n = 512.0
        func column(_ lon: Double) -> Int { Int(floor((lon + 180) / 360 * n)) }
        func row(_ lat: Double) -> Int {
            let rad = lat * .pi / 180
            return Int(floor((1 - log(tan(rad) + 1 / cos(rad)) / .pi) / 2 * n))
        }
        let dLat = radiusKm / kmPerDegree, dLon = radiusKm / (kmPerDegree * cos(start.latitude * .pi / 180))
        return stride(from: column(start.longitude - dLon), through: column(start.longitude + dLon), by: 1).flatMap { x in
            stride(from: row(start.latitude + dLat), through: row(start.latitude - dLat), by: 1).map { y in "9-\(x)-\(y)" }
        }
    }

    private static func passes(_ route: CatalogRecord, _ query: RouteQuery) -> Bool {
        guard query.activity.kinds.contains(route.kind),
              query.covered.map({ covered in route.cells.allSatisfy(covered.contains) }) ?? true,
              query.shape == .any || route.loop == (query.shape == .loop),
              query.distanceKm.contains(route.length_m, scale: 1000), query.climbM.contains(route.ascent_m) else { return false }
        guard let hardest = query.hardest, query.activity == .hiking || query.activity == .mtb else { return true }
        // An ungraded route is easy for the upper bound, but only an explicit grade meets a lower bound.
        return route.hardest ?? 0 <= hardest.upperBound && (hardest.lowerBound == 0 || route.hardest ?? -1 >= hardest.lowerBound)
    }

    /// Distance in km from `point` to the line, or nil beyond `km`, in a flat projection at the latitude of `point`.
    private static func distanceKm(from point: Coordinate, to line: [Coordinate], within km: Double) -> Double? {
        let kx = kmPerDegree * cos(point.latitude * .pi / 180), ky = kmPerDegree
        var nearest = Double.infinity
        for (index, b) in line.enumerated() {
            let a = line[max(0, index - 1)]
            let ax = (a.longitude - point.longitude) * kx, ay = (a.latitude - point.latitude) * ky
            let dx = (b.longitude - a.longitude) * kx, dy = (b.latitude - a.latitude) * ky
            let length = dx * dx + dy * dy
            let t = max(0, min(1, -(ax * dx + ay * dy) / (length == 0 ? 1 : length)))
            let x = ax + t * dx, y = ay + t * dy
            nearest = min(nearest, x * x + y * y)
        }
        return nearest <= km * km ? nearest.squareRoot() : nil
    }
}
