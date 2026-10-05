import Foundation
import OBCDomain

public struct PlannerRelease: Decodable, Equatable, Sendable {
    public let id: String
    public let region: String
    public let bounds: [Double]
    public let basemap: URL
    public let glyphs: String
    public let sprites: String
    public let terrain: String
    public let terrain_attribution: String
    /// The OSM credit of the release, from data/sources.toml. Nil for a release without one.
    public let attribution: String?
    /// The credit of the basemap's land cover. Nil for a release without one.
    public let landcover_attribution: String?
    public let search: URL
    public let routing: URL
    public let manifest: URL
    /// The route network TileJSON.
    public let overlays: URL
    /// The route catalog: a cell file URL with `{cell}`. Nil when the release has none.
    public let routes: String?
    /// The cell IDs of an offline grid selection. Only routes wholly inside them are listed.
    public let offlineCells: [String]?

    public init(id: String, region: String, bounds: [Double], basemap: URL, glyphs: String,
                sprites: String, terrain: String, terrain_attribution: String, search: URL, routing: URL, manifest: URL,
                overlays: URL, routes: String? = nil, offlineCells: [String]? = nil, attribution: String? = nil,
                landcover_attribution: String? = nil) {
        self.id = id; self.region = region; self.bounds = bounds; self.basemap = basemap
        self.glyphs = glyphs; self.sprites = sprites; self.terrain = terrain
        self.terrain_attribution = terrain_attribution; self.search = search; self.routing = routing; self.manifest = manifest
        self.overlays = overlays; self.routes = routes; self.offlineCells = offlineCells; self.attribution = attribution
        self.landcover_attribution = landcover_attribution
    }

    public var isLocal: Bool { manifest.isFileURL }

    public func contains(_ coordinate: Coordinate) -> Bool {
        bounds.count == 4 && (bounds[0]...bounds[2]).contains(coordinate.longitude)
            && (bounds[1]...bounds[3]).contains(coordinate.latitude)
    }
}

public enum RoutePreference: String, CaseIterable, Sendable {
    case balanced, shorter, lessClimbing = "less-climbing"
    public var title: String {
        switch self { case .balanced: "Balanced"; case .shorter: "Shorter"; case .lessClimbing: "Less climbing" }
    }
    public func profile(for activity: RouteActivity) -> String {
        activity.rawValue + (self == .balanced ? "" : "/" + rawValue)
    }
}

/// The planner activities. The raw value is the mode of the routing profile.
public enum RouteActivity: String, CaseIterable, Sendable {
    case road, gravel, mtb, touring, hiking
    public init(_ bike: BikeType) {
        switch bike { case .road: self = .road; case .gravel: self = .gravel; case .mtb: self = .mtb; case .touring: self = .touring }
    }
    public var name: String { bikeType?.name ?? "Hiking" }
    /// The device type of a saved plan. The device has no hiking type.
    public var bikeType: BikeType? {
        switch self { case .road: .road; case .gravel: .gravel; case .mtb: .mtb; case .touring: .touring; case .hiking: nil }
    }
}

public struct PlannedPath: Sendable {
    public let points: [RoutePoint]
    public let distance: Double
    public let ascent: Double
    public let seconds: Double
    public let pointIndices: [Int]
    public let elapsed: [Double]
    public init(points: [RoutePoint], distance: Double, ascent: Double, seconds: Double,
                pointIndices: [Int], elapsed: [Double]) {
        self.points = points; self.distance = distance; self.ascent = ascent; self.seconds = seconds
        self.pointIndices = pointIndices; self.elapsed = elapsed
    }
}

public struct PlannerPlace: Decodable, Identifiable, Sendable {
    public var id: String { source }
    public let source: String
    public let name: String
    public let city: String
    public let kind: String
    public let opening_hours: String?
    public let website: String?
    public let phone: String?
    public let description: String?
    public struct Position: Decodable, Sendable { public let along: Double; public let distance: Double }
    public let position: Position?
    public let lon: Double
    public let lat: Double
    public var coordinate: Coordinate { Coordinate(latitude: lat, longitude: lon) }
}

public enum PlannerFailure: Error, Equatable, Sendable, LocalizedError {
    case unavailable, invalidData, outsideRegion, noRoad, busy, offlineUnavailable
    /// Shape only: the line has too many points or is too long.
    case lineTooLong
    /// Shape only: no plan of at most 64 points follows the line.
    case lineNotReproducible
    public var errorDescription: String? {
        switch self {
        case .unavailable: "The route service is unavailable. Check your connection and try again."
        case .invalidData: "The route data could not be read. Try again."
        case .outsideRegion: "Choose points inside the available map region."
        case .noRoad: "No route connects these points. Move a point to a nearby road and try again."
        case .busy: "The route service is busy. Try again in a moment."
        case .offlineUnavailable: "No usable offline map covers this request, and the online service is unavailable. Check your connection or download this area."
        case .lineTooLong: "The line is too long to plan."
        case .lineNotReproducible: "No route on roads follows the line."
        }
    }
}

public protocol RoutePlanning: Sendable {
    func release() async throws -> PlannerRelease
    /// `turnarounds` are interior point indices where the route turns back on purpose.
    func route(points: [Coordinate], turnarounds: [Int], activity: RouteActivity, preference: RoutePreference,
               release: PlannerRelease) async throws -> PlannedPath
}

/// The plan points of a route that follows a line: a route request through `points` with `turnarounds`
/// follows it. The first and last points are the line ends.
public struct PlannedShape: Equatable, Sendable {
    public let points: [Coordinate]
    public let turnarounds: [Int]
    public init(points: [Coordinate], turnarounds: [Int]) { self.points = points; self.turnarounds = turnarounds }
}

public struct PlannerSearchQuery: Sendable {
    public var text: String
    public var source: String?
    public var view: [Double]?
    public var kinds: [String] = []
    public var route: [Coordinate] = []
    public var alongRoute = false
    public var routeLengthMeters = 0.0
    public var fromMeters = 0.0
    public var toMeters: Double?
    public var radiusMeters: Double?
    public init(text: String, view: [Double]? = nil) { self.text = text; self.view = view }
}

/// The same release supplies maps, search, and routing.
/// A local provider can implement this boundary without changing planner interactions.
public protocol PlannerDataSource: RoutePlanning {
    var supportsOffline: Bool { get }
    func mapRelease(bounds: [Double]?, allowNetwork: Bool) async throws -> PlannerRelease
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace]
    /// The routing profiles of the release, or nil when the source does not know them.
    func profiles(release: PlannerRelease) async throws -> [String]?
    /// The plan points of a route with `profile` that follows `line`.
    func shape(line: [Coordinate], profile: String) async throws -> PlannedShape
}

extension PlannerDataSource {
    public var supportsOffline: Bool { false }
    public func profiles(release: PlannerRelease) async throws -> [String]? { nil }
    public func shape(line: [Coordinate], profile: String) async throws -> PlannedShape { throw PlannerFailure.unavailable }
    public func mapRelease(bounds: [Double]?, allowNetwork: Bool = true) async throws -> PlannerRelease {
        guard allowNetwork else { throw PlannerFailure.offlineUnavailable }
        return try await release()
    }
}

/// Sends the requests of a planning session to one release. When a catalogue switch removes it, the requests
/// move to the new active release.
public actor PlannerService: PlannerDataSource {
    public static let shared = PlannerService()
    private let catalogURL: URL
    public typealias Transport = @Sendable (URLRequest) async throws -> (Data, URLResponse)
    private let transport: Transport
    private let fixedRelease: PlannerRelease?
    private var cached: (release: PlannerRelease, fetched: Date)?
    private var loading: Task<PlannerRelease, Error>?
    private struct Manifest: Decodable { let routing_package: String; let profiles: [String] }
    private var searchLine: (original: [Coordinate], simplified: [Coordinate])?
    /// Routed legs, so an edit requests only the legs that it changed.
    private var legs: [LegKey: Leg] = [:]
    private var manifestCache: (id: String, value: Manifest)?

    private func manifest(_ release: PlannerRelease) async throws -> Manifest {
        if let cached = manifestCache, cached.id == release.id { return cached.value }
        let value = try Self.decode(Manifest.self, data: await Self.get(release.manifest, transport: transport))
        manifestCache = (release.id, value)
        return value
    }

    public init(catalogURL: URL = URL(string: "https://maps.openbikecomputer.com/planner/catalog.json")!,
                session: URLSession = .shared) {
        self.catalogURL = catalogURL
        self.transport = { try await session.data(for: $0) }
        self.fixedRelease = nil
    }

    public init(release: PlannerRelease, transport: @escaping Transport) {
        self.catalogURL = release.manifest
        self.fixedRelease = release
        self.transport = transport
    }

    init(catalogURL: URL, transport: @escaping Transport) {
        self.catalogURL = catalogURL
        self.fixedRelease = nil
        self.transport = transport
    }

    /// A release object answered 404.
    private struct ReleaseGone: Error {}

    /// Release objects are immutable, so a 404 means that a catalogue switch removed the release. The catalogue is read
    /// again, once, and the request repeats with the new active release.
    /// A newer cached active release replaces `release` before the first request.
    private func onActive<T>(_ release: PlannerRelease, _ body: (PlannerRelease) async throws -> T) async throws -> T {
        let first = fixedRelease == nil ? cached?.release ?? release : release
        do { return try await body(first) } catch is ReleaseGone {
            guard fixedRelease == nil else { throw PlannerFailure.unavailable }
            let active = try await reloadedRelease()
            guard active.id != first.id else { throw PlannerFailure.unavailable }
            do { return try await body(active) } catch is ReleaseGone { throw PlannerFailure.unavailable }
        }
    }

    /// The active release, read again from the catalogue.
    public func reloadedRelease() async throws -> PlannerRelease {
        cached = nil
        return try await release()
    }

    public func release() async throws -> PlannerRelease {
        if let fixedRelease { return fixedRelease }
        if let cached, Date().timeIntervalSince(cached.fetched) < 30 { return cached.release }
        if let loading { return try await loading.value }
        let task = Task { [transport, catalogURL] in
            struct Catalog: Decodable { let format: Int; let active: PlannerRelease }
            var request = URLRequest(url: catalogURL)
            request.timeoutInterval = 25
            // A CDN can give the catalogue a browser cache lifetime longer than its own.
            request.cachePolicy = .reloadIgnoringLocalCacheData
            let data: Data
            do { data = try await Self.send(request, transport: transport) } catch is ReleaseGone { throw PlannerFailure.unavailable }
            let catalog = try Self.decode(Catalog.self, data: data)
            let r = catalog.active
            guard catalog.format == 1, r.id.count == 64, r.id.allSatisfy({ $0.isHexDigit && $0.isASCII }), r.bounds.count == 4,
                  r.bounds.allSatisfy(\.isFinite), r.bounds[0] < r.bounds[2], r.bounds[1] < r.bounds[3],
                  r.bounds[0] >= -180, r.bounds[2] <= 180, r.bounds[1] >= -90, r.bounds[3] <= 90,
                  r.basemap.scheme == "https", r.routing.scheme == "https", r.manifest.scheme == "https", r.search.scheme == "https",
                  r.overlays.scheme == "https",
                  r.routes.map({ $0.hasPrefix("https://") && $0.contains("{cell}") }) ?? true,
                  [r.glyphs, r.sprites, r.terrain].allSatisfy({ $0.hasPrefix("https://") })
            else { throw PlannerFailure.invalidData }
            return r
        }
        loading = task
        defer { loading = nil }
        let release = try await task.value
        cached = (release, Date())
        return release
    }

    public func profiles(release: PlannerRelease) async throws -> [String]? {
        try await onActive(release) { try await self.manifest($0).profiles }
    }

    public func route(points: [Coordinate], turnarounds: [Int] = [], activity: RouteActivity, preference: RoutePreference = .balanced,
                      release: PlannerRelease) async throws -> PlannedPath {
        guard (2...64).contains(points.count), points.allSatisfy(release.contains) else {
            throw PlannerFailure.outsideRegion
        }
        guard turnarounds.allSatisfy({ (1..<points.count - 1).contains($0) }) else { throw PlannerFailure.invalidData }
        let profile = preference.profile(for: activity)
        return try await onActive(release) { release in
            let manifest = try await self.manifest(release)
            guard manifest.profiles.contains(profile) else { throw PlannerFailure.invalidData }
            return try await self.route(points, turnarounds: Set(turnarounds), profile: profile, release: release,
                                        package: manifest.routing_package, whole: false)
        }
    }

    /// One request at most: for the legs from the first to the last leg that is not cached, pinned to the cached legs
    /// before and after it. When it finds no route, one request for the whole route follows: the whole route can pass a
    /// neighbour point the other way.
    private func route(_ points: [Coordinate], turnarounds: Set<Int>, profile: String, release: PlannerRelease, package: String,
                       whole: Bool) async throws -> PlannedPath {
        let turn = turnarounds.contains
        let keys = zip(points, points.dropFirst()).enumerated().map { k, leg in
            LegKey(package: package, profile: profile, from: leg.0, to: leg.1, turns: [turn(k), turn(k + 1)])
        }
        // A leg from another request joins only at the same road position, so a shaping point keeps its direction.
        // A turnaround joins legs in either direction.
        func joins(_ a: Leg?, _ b: Leg?, at point: Int) -> Bool { a == nil || b == nil || turn(point) || a!.end == b!.start }
        var found = keys.map { whole ? nil : legs[$0] }
        for k in found.indices.dropFirst() where !joins(found[k - 1], found[k], at: k) { found[k] = nil }
        if var first = found.firstIndex(where: { $0 == nil }), var last = found.lastIndex(where: { $0 == nil }) {
            // A turnaround stays inside the request: only an interior turnaround may depart on the other road.
            if turn(first) { first -= 1 }
            if turn(last + 1) { last += 1 }
            let before = first > 0 ? found[first - 1] : nil, after = last + 1 < found.count ? found[last + 1] : nil
            let fresh: [Leg]
            do {
                let inner = turnarounds.filter { $0 > first && $0 <= last }.map { $0 - first }.sorted()
                fresh = try await request(Array(points[first...last + 1]), turnarounds: inner, profile: profile,
                                          pins: (before?.end, after?.start), release: release, package: package)
            } catch PlannerFailure.noRoad where before != nil || after != nil {
                // Only a missing path can change with the whole route; a busy service must not get a larger request.
                return try await route(points, turnarounds: turnarounds, profile: profile, release: release, package: package, whole: true)
            }
            // The service ignores a pin that is not a road candidate of its point.
            if !joins(before, fresh[0], at: first) || !joins(fresh[fresh.count - 1], after, at: last + 1) {
                legs.removeAll()
                return try await route(points, turnarounds: turnarounds, profile: profile, release: release, package: package, whole: true)
            }
            found.replaceSubrange(first...last, with: fresh as [Leg?])
        }
        let joined = found.compactMap { $0 }
        // About ten 300 km trips. Above that, the cache keeps only this route.
        if legs.values.reduce(0, { $0 + $1.points.count }) > 200_000 { legs.removeAll() }
        for (key, leg) in zip(keys, joined) { legs[key] = leg }
        // A leg starts at the last point of the leg before it.
        var path: [RoutePoint] = [], elapsed: [Double] = [], indices = [0]
        for leg in joined {
            let skip = path.isEmpty ? 0 : 1, offset = elapsed.last ?? 0
            path += leg.points.dropFirst(skip)
            elapsed += leg.elapsed.dropFirst(skip).map { $0 + offset }
            indices.append(path.count - 1)
        }
        return PlannedPath(points: path, distance: joined.reduce(0) { $0 + $1.totals.distance_m },
                           ascent: joined.reduce(0) { $0 + $1.totals.ascent_m }, seconds: joined.reduce(0) { $0 + $1.totals.seconds },
                           pointIndices: indices, elapsed: elapsed)
    }

    /// `turns`: whether the route turns back at the start and at the end of the leg.
    private struct LegKey: Hashable { let package: String; let profile: String; let from: Coordinate; let to: Coordinate; let turns: [Bool] }
    /// One leg cut from a route answer. Its `elapsed` starts at zero.
    private struct Leg { let points: [RoutePoint]; let elapsed: [Double]; let totals: RouteAnswer.Totals; let start: String; let end: String }

    private func request(_ points: [Coordinate], turnarounds: [Int], profile: String, pins: (start: String?, end: String?),
                         release: PlannerRelease, package: String) async throws -> [Leg] {
        struct Query: Encodable {
            let points: [[Double]]; let profile: String; let alternatives = false; let turnarounds: [Int]?
            let start_position: String?; let end_position: String?
        }
        var request = URLRequest(url: release.routing.appending(path: "v1/route"))
        request.httpMethod = "POST"
        request.timeoutInterval = 25
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(Query(points: points.map { [$0.longitude, $0.latitude] }, profile: profile,
                                                          turnarounds: turnarounds.isEmpty ? nil : turnarounds,
                                                          start_position: pins.start, end_position: pins.end))
        let data = try await Self.send(request, transport: transport)
        struct Response: Decodable { let routes: [RouteAnswer] }
        let response = try Self.decode(Response.self, data: data)
        guard let route = response.routes.first, route.package == package, route.profile == profile,
              route.coordinates.count > 1, route.coordinates.count <= 250_000,
              route.elevation.count == route.coordinates.count,
              route.elapsed.count == route.coordinates.count, route.elapsed.first == 0,
              zip(route.elapsed, route.elapsed.dropFirst()).allSatisfy({ $0 <= $1 }),
              route.legs.count == points.count - 1, route.legs.first?.from_index == 0,
              route.legs.last?.to_index == route.coordinates.count - 1,
              route.legs.allSatisfy({ $0.from_index >= 0 && $0.to_index >= $0.from_index && $0.to_index < route.coordinates.count }),
              zip(route.legs, route.legs.dropFirst()).allSatisfy({ $0.to_index == $1.from_index }),
              route.coordinates.allSatisfy({ (-180...180).contains($0.longitude) && (-90...90).contains($0.latitude) }),
              route.legs.allSatisfy({ [$0.totals.distance_m, $0.totals.ascent_m, $0.totals.seconds].allSatisfy { $0.isFinite && $0 >= 0 } })
        else { throw PlannerFailure.invalidData }
        try Task.checkCancellation()
        return route.legs.map { leg in
            let range = leg.from_index...leg.to_index
            return Leg(points: range.map { RoutePoint(coordinate: route.coordinates[$0], elevationMeters: route.elevation[$0]) },
                       elapsed: route.elapsed[range].map { $0 - route.elapsed[leg.from_index] },
                       totals: leg.totals, start: leg.start, end: leg.end)
        }
    }

    /// The service's caps on a shape line, after the simplification.
    static let shapeMaxPoints = 2_000, shapeMaxMeters = 200_000.0

    /// One request. The line is simplified within 10 m and sent with 6 decimals, so a 200 km line fits the body limit.
    /// A line over the service's caps fails without a request.
    public func shape(line: [Coordinate], profile: String) async throws -> PlannedShape {
        let simplified = RideMapLine(id: RideID("planner-shape"), pieces: [line]).simplified(toleranceMeters: 10).pieces.first ?? []
        guard simplified.count > 1 else { throw PlannerFailure.invalidData }
        guard simplified.count <= Self.shapeMaxPoints,
              zip(simplified, simplified.dropFirst()).reduce(0, { $0 + $1.0.distance(to: $1.1) }) <= Self.shapeMaxMeters
        else { throw PlannerFailure.lineTooLong }
        struct Query: Encodable { let line: [[Double]]; let profile: String }
        func rounded(_ value: Double) -> Double { (value * 1e6).rounded() / 1e6 }
        let body = try JSONEncoder().encode(Query(line: simplified.map { [rounded($0.longitude), rounded($0.latitude)] }, profile: profile))
        struct Answer: Decodable { let points: [[Double]]; let turnarounds: [Int] }
        let answer = try await onActive(try await release()) { release in
            var request = URLRequest(url: release.routing.appending(path: "v1/shape"))
            request.httpMethod = "POST"
            // The service stops a shape after 30 s.
            request.timeoutInterval = 40
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
            return try Self.decode(Answer.self, data: await Self.send(request, transport: self.transport))
        }
        guard (2...64).contains(answer.points.count),
              answer.points.allSatisfy({ $0.count == 2 && (-180...180).contains($0[0]) && (-90...90).contains($0[1]) }),
              answer.turnarounds.allSatisfy({ (1..<answer.points.count - 1).contains($0) })
        else { throw PlannerFailure.invalidData }
        try Task.checkCancellation()
        return PlannedShape(points: answer.points.map { Coordinate(latitude: $0[1], longitude: $0[0]) }, turnarounds: answer.turnarounds)
    }

    public func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] {
        try await onActive(release) { try await self.search(query, on: $0) }
    }

    private func search(_ query: PlannerSearchQuery, on release: PlannerRelease) async throws -> [PlannerPlace] {
        if query.kinds.count > 3 {
            var results: [PlannerPlace] = [], ids: Set<String> = []
            for start in stride(from: 0, to: query.kinds.count, by: 3) {
                try Task.checkCancellation()
                var part = query
                part.kinds = Array(query.kinds[start..<min(start + 3, query.kinds.count)])
                for place in try await search(part, on: release) where ids.insert(place.id).inserted { results.append(place) }
            }
            return Array(results.prefix(100))
        }
        let kinds = query.kinds, route = searchCoordinates(query.route), view = query.view
        let fromMeters = query.fromMeters, toMeters = query.toMeters, radiusMeters = query.radiusMeters
        let text = query.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.count <= 240 else { return [] }
        var request = URLRequest(url: release.search.appending(path: "query"))
        request.httpMethod = "POST"
        request.timeoutInterval = 25
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var queryRequest: [String: Any] = kinds.isEmpty
            ? ["type": "place", "name": text] : ["type": "places", "what": kinds]
        var context: [String: Any] = ["coordinates": route.map { [$0.longitude, $0.latitude] }, "days": [], "points": []]
        if !route.isEmpty {
            context["days"] = [["number": 1, "from": 0, "to": query.routeLengthMeters / 1000, "rest": false]]
            var along: [String: Any] = ["ref": "km", "from": ["value": fromMeters / 1000, "unit": "km"]]
            if let toMeters { along["to"] = ["value": toMeters / 1000, "unit": "km"] }
            queryRequest["where"] = query.alongRoute ? ["scope": "route", "along": along] : ["scope": "view"]
        } else { queryRequest["where"] = ["scope": "view"] }
        if kinds.isEmpty { queryRequest.removeValue(forKey: "where") }
        if let radiusMeters, !kinds.isEmpty { queryRequest["radius"] = ["value": radiusMeters / 1000, "unit": "km"] }
        var body: [String: Any] = ["q": text, "view": view ?? release.bounds, "plan": context, "limit": 100,
                                  "request": queryRequest]
        if let source = query.source { body["source"] = source }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        struct Response: Decodable { let results: [PlannerPlace] }
        let data = try await Self.send(request, transport: transport)
        let result = try Self.decode(Response.self, data: data)
        guard result.results.count <= 100, Set(result.results.map(\.id)).count == result.results.count,
              result.results.allSatisfy({ place in
                  (query.source.map { $0 == place.source } ?? true)
                    && place.lat.isFinite && place.lon.isFinite && (-90...90).contains(place.lat) && (-180...180).contains(place.lon)
                    && (place.position.map { $0.along.isFinite && $0.distance.isFinite && $0.along >= 0 && $0.distance >= 0 } ?? true)
              }) else { throw PlannerFailure.invalidData }
        try Task.checkCancellation()
        return result.results
    }

    private func searchCoordinates(_ coordinates: [Coordinate]) -> [Coordinate] {
        guard coordinates.count > 20_000 else { return coordinates }
        if let searchLine, searchLine.original == coordinates { return searchLine.simplified }
        var line = RideMapLine(id: RideID("planner-search"), pieces: [coordinates])
        var tolerance = 1.0
        while (line.pieces.first?.count ?? 0) > 20_000 {
            line = line.simplified(toleranceMeters: tolerance)
            tolerance *= 2
        }
        let simplified = line.pieces.first ?? []
        searchLine = (coordinates, simplified)
        return simplified
    }

    private static func get(_ url: URL, transport: Transport) async throws -> Data {
        var request = URLRequest(url: url)
        request.timeoutInterval = 25
        return try await send(request, transport: transport)
    }

    private static func decode<T: Decodable>(_ type: T.Type, data: Data) throws -> T {
        do { return try JSONDecoder().decode(type, from: data) }
        catch { throw PlannerFailure.invalidData }
    }

    private static func send(_ request: URLRequest, transport: Transport) async throws -> Data {
        do {
            let (data, response) = try await transport(request)
            try Task.checkCancellation()
            guard let response = response as? HTTPURLResponse else { throw PlannerFailure.invalidData }
            if response.statusCode == 404 { throw ReleaseGone() }
            guard response.statusCode == 200 else {
                struct Failure: Decodable { let code: String }
                let code = try? JSONDecoder().decode(Failure.self, from: data).code
                switch code {
                case "no_snap", "no_path": throw PlannerFailure.noRoad
                case "missing_region": throw PlannerFailure.outsideRegion
                case "busy", "limit", "cancelled": throw PlannerFailure.busy
                case "line_too_long": throw PlannerFailure.lineTooLong
                case "line_not_reproducible": throw PlannerFailure.lineNotReproducible
                default: throw PlannerFailure.unavailable
                }
            }
            return data
        } catch is CancellationError { throw CancellationError() }
        catch let error as URLError where error.code == .cancelled { throw CancellationError() }
        catch let error as PlannerFailure { throw error }
        catch let error as ReleaseGone { throw error }
        catch { throw PlannerFailure.unavailable }
    }
}
