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
    public let search: URL
    public let routing: URL
    public let manifest: URL

    public init(id: String, region: String, bounds: [Double], basemap: URL, glyphs: String,
                sprites: String, terrain: String, terrain_attribution: String, search: URL, routing: URL, manifest: URL) {
        self.id = id; self.region = region; self.bounds = bounds; self.basemap = basemap
        self.glyphs = glyphs; self.sprites = sprites; self.terrain = terrain
        self.terrain_attribution = terrain_attribution; self.search = search; self.routing = routing; self.manifest = manifest
    }

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
    public func profile(for bike: BikeType) -> String {
        let mode = switch bike { case .road: "road"; case .gravel: "gravel"; case .mtb: "mtb"; case .touring: "touring" }
        return mode + (self == .balanced ? "" : "/" + rawValue)
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
    public struct Position: Decodable, Sendable { public let along: Double; public let distance: Double }
    public let position: Position?
    public let lon: Double
    public let lat: Double
    public var coordinate: Coordinate { Coordinate(latitude: lat, longitude: lon) }
}

public enum PlannerFailure: Error, Equatable, Sendable, LocalizedError {
    case unavailable, invalidData, outsideRegion, noRoad, busy
    public var errorDescription: String? {
        switch self {
        case .unavailable: "The route service is unavailable. Check your connection and try again."
        case .invalidData: "The route data could not be read. Try again."
        case .outsideRegion: "Choose points inside the available map region."
        case .noRoad: "No route connects these points. Move a point to a nearby road and try again."
        case .busy: "The route service is busy. Try again in a moment."
        }
    }
}

public protocol RoutePlanning: Sendable {
    func release() async throws -> PlannerRelease
    func route(points: [Coordinate], bike: BikeType, preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath
}

public struct PlannerSearchQuery: Sendable {
    public var text: String
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

/// The same release supplies maps, search, routing, and viewport overlays.
/// A local provider can implement this boundary without changing planner interactions.
public protocol PlannerDataSource: RoutePlanning {
    func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace]
    func overlays(bounds: [Double], zoom: Double, network: String, release: PlannerRelease) async throws -> Data
}

/// Pins every request in a planning session to the release that supplies its map.
public actor PlannerService: PlannerDataSource {
    public static let shared = PlannerService()
    private let catalogURL: URL
    private let session: URLSession
    private var cached: (release: PlannerRelease, fetched: Date)?
    private var loading: Task<PlannerRelease, Error>?
    private struct Manifest: Decodable { let routing_package: String; let profiles: [String] }
    private var searchLine: (original: [Coordinate], simplified: [Coordinate])?
    private var manifestCache: (id: String, value: Manifest)?

    private func manifest(_ release: PlannerRelease) async throws -> Manifest {
        if let cached = manifestCache, cached.id == release.id { return cached.value }
        let value = try Self.decode(Manifest.self, data: await Self.get(release.manifest, session: session))
        manifestCache = (release.id, value)
        return value
    }

    public init(catalogURL: URL = URL(string: "https://maps.openbikecomputer.com/planner/catalog.json")!,
                session: URLSession = .shared) {
        self.catalogURL = catalogURL
        self.session = session
    }

    public func release() async throws -> PlannerRelease {
        if let cached, Date().timeIntervalSince(cached.fetched) < 30 { return cached.release }
        if let loading { return try await loading.value }
        let task = Task { [session, catalogURL] in
            struct Catalog: Decodable { let format: Int; let active: PlannerRelease }
            let data = try await Self.get(catalogURL, session: session)
            let catalog = try Self.decode(Catalog.self, data: data)
            let r = catalog.active
            guard catalog.format == 1, r.id.count == 64, r.id.allSatisfy({ $0.isHexDigit && $0.isASCII }), r.bounds.count == 4,
                  r.bounds.allSatisfy(\.isFinite), r.bounds[0] < r.bounds[2], r.bounds[1] < r.bounds[3],
                  r.bounds[0] >= -180, r.bounds[2] <= 180, r.bounds[1] >= -90, r.bounds[3] <= 90,
                  r.basemap.scheme == "https", r.routing.scheme == "https", r.manifest.scheme == "https", r.search.scheme == "https",
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

    public func route(points: [Coordinate], bike: BikeType, preference: RoutePreference = .balanced,
                      release: PlannerRelease) async throws -> PlannedPath {
        guard (2...64).contains(points.count), points.allSatisfy(release.contains) else {
            throw PlannerFailure.outsideRegion
        }
        let manifest = try await manifest(release)
        let profile = preference.profile(for: bike)
        guard manifest.profiles.contains(profile) else { throw PlannerFailure.invalidData }
        struct Query: Encodable { let points: [[Double]]; let profile: String; let alternatives = false }
        var request = URLRequest(url: release.routing.appending(path: "v1/route"))
        request.httpMethod = "POST"
        request.timeoutInterval = 25
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(Query(points: points.map { [$0.longitude, $0.latitude] }, profile: profile))
        let data = try await Self.send(request, session: session)
        struct Response: Decodable { let routes: [Route] }
        struct Route: Decodable {
            let package: String
            let profile: String
            let geometry: [[Double]]
            let elevation: [Double?]
            let elapsed: [Double]
            struct Leg: Decodable { let from_index: Int; let to_index: Int }
            let legs: [Leg]
            let totals: Totals
        }
        struct Totals: Decodable { let distance_m: Double; let ascent_m: Double; let seconds: Double }
        let response = try Self.decode(Response.self, data: data)
        guard let route = response.routes.first, route.package == manifest.routing_package, route.profile == profile,
              route.geometry.count > 1, route.geometry.count <= 250_000,
              route.elevation.count == route.geometry.count,
              route.elapsed.count == route.geometry.count, route.elapsed.first == 0,
              route.elapsed.allSatisfy({ $0.isFinite && $0 >= 0 }),
              zip(route.elapsed, route.elapsed.dropFirst()).allSatisfy({ $0 <= $1 }),
              route.legs.count == points.count - 1, route.legs.first?.from_index == 0,
              route.legs.last?.to_index == route.geometry.count - 1,
              route.legs.allSatisfy({ $0.from_index >= 0 && $0.to_index >= $0.from_index && $0.to_index < route.geometry.count }),
              zip(route.legs, route.legs.dropFirst()).allSatisfy({ $0.to_index == $1.from_index }),
              route.geometry.allSatisfy({ $0.count == 2 && $0.allSatisfy(\.isFinite) && (-180...180).contains($0[0]) && (-90...90).contains($0[1]) }),
              route.elevation.allSatisfy({ $0?.isFinite ?? true }),
              [route.totals.distance_m, route.totals.ascent_m, route.totals.seconds].allSatisfy({ $0.isFinite && $0 >= 0 })
        else { throw PlannerFailure.invalidData }
        try Task.checkCancellation()
        return PlannedPath(points: zip(route.geometry, route.elevation).map {
            RoutePoint(coordinate: Coordinate(latitude: $0.0[1], longitude: $0.0[0]), elevationMeters: $0.1)
        }, distance: route.totals.distance_m, ascent: route.totals.ascent_m, seconds: route.totals.seconds,
           pointIndices: [0] + route.legs.map(\.to_index), elapsed: route.elapsed)
    }

    public func search(_ query: PlannerSearchQuery, release: PlannerRelease) async throws -> [PlannerPlace] {
        if query.kinds.count > 3 {
            var results: [PlannerPlace] = [], ids: Set<String> = []
            for start in stride(from: 0, to: query.kinds.count, by: 3) {
                try Task.checkCancellation()
                var part = query
                part.kinds = Array(query.kinds[start..<min(start + 3, query.kinds.count)])
                for place in try await search(part, release: release) where ids.insert(place.id).inserted { results.append(place) }
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
        let body: [String: Any] = ["q": text, "region": release.region, "view": view ?? release.bounds,
                                  "plan": context, "limit": 100, "submitted": true, "request": queryRequest]
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        struct Response: Decodable { let results: [PlannerPlace] }
        let data = try await Self.send(request, session: session)
        let result = try Self.decode(Response.self, data: data)
        guard result.results.count <= 100, Set(result.results.map(\.id)).count == result.results.count,
              result.results.allSatisfy({ place in
                  place.lat.isFinite && place.lon.isFinite && (-90...90).contains(place.lat) && (-180...180).contains(place.lon)
                    && (place.position.map { $0.along.isFinite && $0.distance.isFinite && $0.along >= 0 && $0.distance >= 0 } ?? true)
              }) else { throw PlannerFailure.invalidData }
        try Task.checkCancellation()
        return result.results
    }

    public func overlays(bounds: [Double], zoom: Double, network: String, release: PlannerRelease) async throws -> Data {
        guard bounds.count == 4, bounds.allSatisfy(\.isFinite), bounds[0] < bounds[2], bounds[1] < bounds[3], zoom.isFinite,
              ["cycling", "hiking"].contains(network) else { throw PlannerFailure.invalidData }
        var url = URLComponents(url: release.routing.appending(path: "v1/overlays"), resolvingAgainstBaseURL: false)!
        url.queryItems = [URLQueryItem(name: "bbox", value: bounds.map { String($0) }.joined(separator: ",")),
                          URLQueryItem(name: "zoom", value: String(min(22, max(6, floor(zoom))))),
                          URLQueryItem(name: "layers", value: network),
                          URLQueryItem(name: "mode", value: network == "hiking" ? "walking" : "cycling")]
        let data = try await Self.get(url.url!, session: session)
        struct Collection: Codable {
            let package: String
            let type: String
            struct Feature: Codable {
                let type: String
                struct Geometry: Codable { let type: String; let coordinates: [[Double]] }
                struct Properties: Codable { let kind: String; let rank: Int; let ref: String }
                let geometry: Geometry
                let properties: Properties
            }
            let features: [Feature]
        }
        let collection = try Self.decode(Collection.self, data: data)
        guard collection.type == "FeatureCollection", collection.package == (try await manifest(release)).routing_package,
              collection.features.reduce(0, { $0 + $1.geometry.coordinates.count }) <= 200_000,
              collection.features.allSatisfy({ feature in
                  feature.type == "Feature" && feature.geometry.type == "LineString" && feature.geometry.coordinates.count >= 2
                    && feature.geometry.coordinates.allSatisfy { $0.count == 2 && $0.allSatisfy(\.isFinite)
                        && (-180...180).contains($0[0]) && (-90...90).contains($0[1]) }
              }) else { throw PlannerFailure.invalidData }
        try Task.checkCancellation()
        // Native workers need geometry and style properties, not the relation catalogue or tags.
        return try JSONEncoder().encode(collection)
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

    private static func get(_ url: URL, session: URLSession) async throws -> Data {
        var request = URLRequest(url: url)
        request.timeoutInterval = 25
        return try await send(request, session: session)
    }

    private static func decode<T: Decodable>(_ type: T.Type, data: Data) throws -> T {
        do { return try JSONDecoder().decode(type, from: data) }
        catch { throw PlannerFailure.invalidData }
    }

    private static func send(_ request: URLRequest, session: URLSession) async throws -> Data {
        do {
            let (data, response) = try await session.data(for: request)
            try Task.checkCancellation()
            guard let response = response as? HTTPURLResponse else { throw PlannerFailure.invalidData }
            guard response.statusCode == 200 else {
                struct Failure: Decodable { let code: String }
                let code = try? JSONDecoder().decode(Failure.self, from: data).code
                switch code {
                case "no_snap", "no_path": throw PlannerFailure.noRoad
                case "missing_region": throw PlannerFailure.outsideRegion
                case "busy", "limit", "cancelled": throw PlannerFailure.busy
                default: throw PlannerFailure.unavailable
                }
            }
            return data
        } catch is CancellationError { throw CancellationError() }
        catch let error as URLError where error.code == .cancelled { throw CancellationError() }
        catch let error as PlannerFailure { throw error }
        catch { throw PlannerFailure.unavailable }
    }
}

public struct OnlineLegRouter: LegRouter {
    private let service: any RoutePlanning
    public init(service: any RoutePlanning = PlannerService.shared) { self.service = service }
    public func route(from: Coordinate, to: Coordinate, bikeType: BikeType,
                      onDownload: @escaping @Sendable () -> Void) async throws -> [RoutePoint] {
        do {
            let release = try await service.release()
            return try await service.route(points: [from, to], bike: bikeType, preference: .balanced, release: release).points
        } catch is CancellationError { throw CancellationError() }
        catch PlannerFailure.noRoad { throw LegRouteFailure.noRoad }
        catch PlannerFailure.outsideRegion { throw LegRouteFailure.noMap }
        catch PlannerFailure.unavailable { throw LegRouteFailure.noConnection }
        catch { throw LegRouteFailure.mapData }
    }
}
