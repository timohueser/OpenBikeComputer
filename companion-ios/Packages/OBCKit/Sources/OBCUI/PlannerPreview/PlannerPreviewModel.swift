import Foundation
import Observation
import OBCDomain
import OBCPlanner

public struct PlannerPreviewPlace: Identifiable, Equatable, Sendable {
    public enum Kind: String, Sendable {
        case town, cafe, water, camping, shop, hotel, shelter, rest, food, toilets, bike, pharmacy, station, viewpoint, peak
        public var title: String {
            switch self { case .town: "Place"; case .cafe: "Café"; case .water: "Water"; case .camping: "Camping"; case .shop: "Food shop"
            case .hotel: "Lodging"; case .shelter: "Shelter"; case .rest: "Rest stop"; case .food: "Food"; case .toilets: "Toilets"
            case .bike: "Bike service"; case .pharmacy: "Pharmacy & hospital"; case .station: "Station"; case .viewpoint: "Viewpoint"; case .peak: "Peak" }
        }
        public var symbol: String {
            switch self { case .town: "mappin"; case .cafe: "cup.and.saucer.fill"; case .water: "drop.fill"; case .camping: "tent.fill"; case .shop: "cart.fill"
            case .hotel: "bed.double"; case .shelter: "house"; case .rest: "table.furniture"; case .food: "fork.knife"; case .toilets: "toilet"
            case .bike: "bicycle"; case .pharmacy: "cross.case"; case .station: "tram"; case .viewpoint: "eye"; case .peak: "mountain.2" }
        }
    }
    public let id: String
    public let name: String
    public let coordinate: Coordinate
    public let kind: Kind
    public let alongRouteMeters: Double
    public let offRouteMeters: Double
    /// Opening hours in the OpenStreetMap `opening_hours` form, as the place index delivers them.
    public let hours: String?
    /// One fact worth a line, such as "Drinking water".
    public let note: String?

    public init(id: String, name: String, coordinate: Coordinate, kind: Kind = .town,
                alongRouteMeters: Double = 0, offRouteMeters: Double = 0, hours: String? = nil, note: String? = nil) {
        self.id = id; self.name = name; self.coordinate = coordinate; self.kind = kind
        self.alongRouteMeters = alongRouteMeters; self.offRouteMeters = offRouteMeters
        self.hours = hours; self.note = note
    }
}

public enum PlannerPreviewPreset: String, CaseIterable, Sendable {
    case balanced, shorter, smoother, lessClimbing
    public var title: String { self == .lessClimbing ? "Less climbing" : rawValue.capitalized }
}

public enum PlannerPreviewPointKind: String, CaseIterable, Sendable {
    case visit, shape, marker
    public var title: String { rawValue.capitalized }
    public var symbol: String {
        switch self { case .visit: "flag"; case .shape: "point.topleft.down.to.point.bottomright.curvepath"; case .marker: "mappin" }
    }
}

public struct PlannerPreviewPoint: Identifiable, Equatable, Sendable {
    public let id: String
    public var place: PlannerPreviewPlace
    public var kind: PlannerPreviewPointKind
    /// The route turns back here on purpose, as a signed route's plan says.
    public var turnaround = false

    public init(place: PlannerPreviewPlace, kind: PlannerPreviewPointKind = .visit, id: String? = nil) {
        self.id = id ?? place.id; self.place = place; self.kind = kind
    }
}

public enum PlannerPreviewAction: Equatable, Sendable {
    case createSample, reverse, splitDays
    public var title: String {
        switch self { case .createSample: "Create this route"; case .reverse: "Reverse route"; case .splitDays: "Split into two days" }
    }
}

public struct PlannerPreviewQueryResult: Sendable {
    public let title: String
    public let explanation: String
    public let places: [PlannerPreviewPlace]
    public let action: PlannerPreviewAction?
}

public struct PlannerPreviewStats: Equatable, Sendable {
    public let distanceMeters: Double
    public let ascentMeters: Double
    public let seconds: Double
}

/// Editing intent is reversible. Geometry comes from the published route service.
@MainActor @Observable
public final class PlannerPreviewModel {
    private struct State: Equatable {
        var points: [PlannerPreviewPoint] = []
        var markers: [PlannerPreviewPoint] = []
        var activity = RouteActivity.gravel
        var preset: PlannerPreviewPreset = .balanced
        var overnightPointID: String?
        /// The finish is the start: the route returns to the first point, and there is no finish point.
        var loop = false
        /// The title of a plan from a signed route.
        var name: String?
        /// Where a new stop goes: before the finish, or at the end of a loop.
        var stopEnd: Int { loop || points.count < 2 ? points.count : points.count - 1 }
        var coordinates: [Coordinate] { (points + (loop ? points.prefix(1) : [])).map(\.place.coordinate) }
        /// Only an interior point turns back, so a loop start sends its turnaround once the start moves.
        var turnarounds: [Int] { points.indices.filter { points[$0].turnaround && $0 > 0 && $0 < coordinates.count - 1 } }
    }
    private var state = State()
    private var past: [State] = []
    private var future: [State] = []

    @ObservationIgnored public let service: any PlannerDataSource
    public private(set) var release: PlannerRelease?
    public private(set) var routingRevision = 0
    public private(set) var isRouting = false
    public private(set) var routeError: String?
    public var mapPlaces: [PlannerPreviewPlace] = []
    private var path: PlannedPath?
    public private(set) var geometry: [Coordinate] = []
    var profile = PlannerPreviewGrade(routePoints: [])
    public private(set) var routeLine = MeasuredLine(routePoints: [])

    public init(sample: Bool = false, service: any PlannerDataSource = PlannerService.shared) {
        self.service = service
        if sample { state = Self.sampleState }
    }

    private struct RoutingKey: Equatable {
        let coordinates: [Coordinate]
        let turnarounds: [Int]
        let activity: RouteActivity
        let preset: PlannerPreviewPreset
    }
    private var routingKey: RoutingKey {
        .init(coordinates: state.coordinates, turnarounds: state.turnarounds, activity: activity, preset: preset)
    }
    public var canSave: Bool { path != nil && !isRouting && routeError == nil }
    public func retryRoute() { routingRevision += 1 }

    public func calculateRoute() async {
        let revision = routingRevision, key = routingKey
        isRouting = hasRoute; routeError = nil
        defer { if revision == routingRevision { isRouting = false } }
        do {
            let selected: PlannerRelease
            if let release { selected = release } else { selected = try await service.release() }
            try Task.checkCancellation()
            guard revision == routingRevision else { return }
            release = selected
            guard hasRoute else { return }
            guard let preference = RoutePreference(rawValue: preset == .lessClimbing ? "less-climbing" : preset.rawValue) else {
                throw PlannerFailure.invalidData
            }
            let result = try await service.route(points: key.coordinates, turnarounds: key.turnarounds, activity: key.activity,
                                                 preference: preference, release: selected)
            try Task.checkCancellation()
            guard revision == routingRevision else { return }
            path = result
            routeLine = MeasuredLine(routePoints: result.points)
            geometry = result.points.map(\.coordinate)
            profile = PlannerPreviewGrade(routePoints: result.points)
        } catch is CancellationError {} catch {
            guard revision == routingRevision else { return }
            path = nil; routeLine = MeasuredLine(routePoints: []); geometry = []; profile = PlannerPreviewGrade(routePoints: [])
            routeError = error.localizedDescription
        }
    }

    public func positionedPlace(_ place: PlannerPreviewPlace) -> PlannerPreviewPlace {
        guard routeLine.length > 0 else { return place }
        let projection = routeLine.projection(of: place.coordinate, near: routeLine.length / 2, window: routeLine.length)
        return .init(id: place.id, name: place.name, coordinate: place.coordinate, kind: place.kind,
                     alongRouteMeters: projection.distance, offRouteMeters: projection.error,
                     hours: place.hours, note: place.note)
    }

    public func searchPlaces(_ query: PlannerSearchQuery) async throws -> [PlannerPreviewPlace] {
        let selected: PlannerRelease
        if let release { selected = release } else { selected = try await service.release() }
        try Task.checkCancellation()
        release = selected
        let places = try await service.search(query, release: selected)
        try Task.checkCancellation()
        let line = routeLine
        return places.map { place in
            let kind = NativePlaceKind.kind(for: place.kind)
            let projection = place.position.map { (distance: $0.along * 1000, error: $0.distance * 1000) }
                ?? line.projection(of: place.coordinate, near: line.length / 2, window: line.length)
            return .init(id: place.source, name: place.name, coordinate: place.coordinate, kind: kind,
                         alongRouteMeters: projection.distance, offRouteMeters: projection.error,
                         hours: place.opening_hours, note: place.city.isEmpty ? nil : place.city)
        }
    }

    private func invalidateRoute(from key: RoutingKey) {
        guard key != routingKey else { return }
        routingRevision += 1
        path = nil; routeLine = MeasuredLine(routePoints: []); geometry = []; profile = PlannerPreviewGrade(routePoints: [])
        routeError = nil; isRouting = hasRoute
    }

    public var points: [PlannerPreviewPoint] { state.points }
    public var markers: [PlannerPreviewPoint] { state.markers }
    public var start: PlannerPreviewPlace? { points.first?.place }
    /// A loop's finish is its start.
    public var finish: PlannerPreviewPlace? { isLoop ? start : points.count > 1 ? points.last?.place : nil }
    public var isLoop: Bool { state.loop }
    /// The start, and the finish of a plan that is not a loop. Only the other points have a kind.
    public func isEndpoint(_ id: String) -> Bool { id == points.first?.id || (!isLoop && id == points.last?.id) }
    public var activity: RouteActivity { state.activity }
    public var preset: PlannerPreviewPreset { state.preset }
    public var overnightPointID: String? { state.overnightPointID }
    public var overnight: PlannerPreviewPlace? { points.first { $0.id == overnightPointID }?.place }
    public var hasRoute: Bool { points.count > 1 }
    public var canUndo: Bool { !past.isEmpty }
    /// The number of edits that undo can take back. It changes with every edit, undo and redo.
    public var undoDepth: Int { past.count }
    public var planName: String? { state.name }
    public var canRedo: Bool { !future.isEmpty }
    public var dayCount: Int { overnight == nil ? 1 : 2 }
    public var routeTitle: String {
        guard let start, let finish else { return "New route" }
        if let name = state.name { return name }
        return isLoop ? "Loop from \(start.name)" : "\(start.name) → \(finish.name)"
    }
    public var routePoints: [RoutePoint] { path?.points ?? [] }
    public var pointDistances: [String: Double] {
        guard let path else { return [:] }
        return Dictionary(uniqueKeysWithValues: zip(points, path.pointIndices).map {
            ($0.0.id, routeLine.vertices[$0.1].distance)
        })
    }
    public var stats: PlannerPreviewStats {
        .init(distanceMeters: path?.distance ?? 0, ascentMeters: path?.ascent ?? 0, seconds: path?.seconds ?? 0)
    }
    public var dayStats: [PlannerPreviewStats] {
        guard let path, let id = overnightPointID, let position = points.firstIndex(where: { $0.id == id }) else { return [stats] }
        let index = path.pointIndices[position], split = routeLine.vertices[index].distance
        let fraction = routeLine.length > 0 ? split / routeLine.length : 0
        let ascent = routeLine.climb(from: 0, to: split)
        return [.init(distanceMeters: path.distance * fraction, ascentMeters: min(path.ascent, ascent), seconds: path.elapsed[index]),
                .init(distanceMeters: path.distance * (1 - fraction), ascentMeters: max(0, path.ascent - ascent), seconds: max(0, path.seconds - path.elapsed[index]))]
    }

    public func exportRoute(name: String) -> ImportedRoute {
        let distances = pointDistances
        let located = points.filter { !isEndpoint($0.id) && $0.kind == .visit }.map { ($0.place, distances[$0.id] ?? 0) }
        let waypoints = located.enumerated().map { index, entry in
            let category: WaypointCategory? = switch entry.0.kind {
            case .water: .water
            case .camping: .campsite
            case .shop: .resupply
            default: nil
            }
            return Waypoint(index: index, name: entry.0.name, note: entry.0.note,
                            distanceAlongMeters: entry.1, coordinate: entry.0.coordinate, category: category)
        }
        let title = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return ImportedRoute(name: title.isEmpty ? routeTitle : title,
                             creator: "OpenBikeComputer", points: routePoints, waypoints: waypoints)
    }

    public func newRoute() { edit { $0 = State() } }
    public func loadSample() { edit { $0 = Self.sampleState } }
    public func setStart(_ place: PlannerPreviewPlace) {
        edit { if $0.points.isEmpty { $0.points.append(Self.newPoint(place, in: $0)) } else { $0.points[0].place = place } }
    }
    /// In a loop, a new finish opens the loop: the route ends there instead of at the start.
    public func setFinish(_ place: PlannerPreviewPlace) {
        edit {
            if $0.points.count < 2 || $0.loop {
                $0.loop = false
                $0.points.append(Self.newPoint(place, in: $0))
            }
            else { $0.points[$0.points.count - 1].place = place }
        }
    }
    public func setActivity(_ activity: RouteActivity) { edit { $0.activity = activity } }
    public func setPreset(_ preset: PlannerPreviewPreset) { edit { $0.preset = preset } }
    public func setOvernight(_ place: PlannerPreviewPlace?) {
        guard hasRoute || place == nil else { return }
        edit { next in
            guard let place else { next.overnightPointID = nil; return }
            if let index = next.points.firstIndex(where: { $0.place.id == place.id }) {
                next.points[index].kind = .visit
                next.overnightPointID = next.points[index].id
            } else {
                let marker = next.markers.first { $0.place.id == place.id }
                let point = marker.map { PlannerPreviewPoint(place: place, id: $0.id) } ?? Self.newPoint(place, in: next)
                next.markers.removeAll { $0.id == point.id }
                next.points.insert(point, at: next.stopEnd)
                next.overnightPointID = point.id
            }
        }
    }
    public func setOvernightPoint(id: String?) {
        edit { next in
            next.overnightPointID = id
            if let index = next.points.firstIndex(where: { $0.id == id }) { next.points[index].kind = .visit }
        }
    }
    public func addPoint(_ place: PlannerPreviewPlace, kind: PlannerPreviewPointKind = .visit) {
        guard !(points + markers).contains(where: { $0.place.id == place.id }) else { return }
        edit {
            let point = Self.newPoint(place, kind: kind, in: $0)
            if kind == .marker { $0.markers.append(point) }
            else { $0.points.insert(point, at: $0.stopEnd) }
        }
    }
    public func removePoint(id: String) {
        edit { $0.points.removeAll { $0.id == id }; $0.markers.removeAll { $0.id == id } }
    }
    public func replacePoint(id: String, with place: PlannerPreviewPlace) {
        guard !(points + markers).contains(where: { $0.place.id == place.id }) else { return }
        edit {
            if let index = $0.points.firstIndex(where: { $0.id == id }) { $0.points[index].place = place }
            if let index = $0.markers.firstIndex(where: { $0.id == id }) { $0.markers[index].place = place }
        }
    }
    public func setPointKind(id: String, kind: PlannerPreviewPointKind) {
        guard var point = (points + markers).first(where: { $0.id == id }), point.kind != kind else { return }
        point.kind = kind
        edit { next in
            if let index = next.points.firstIndex(where: { $0.id == id }) {
                if kind == .marker { next.points.remove(at: index); next.markers.append(point) }
                else { next.points[index] = point }
            } else {
                next.markers.removeAll { $0.id == id }
                next.points.insert(point, at: next.stopEnd)
            }
        }
    }
    /// The start of a loop stays first.
    public func movePoint(fromOffsets offsets: IndexSet, toOffset destination: Int) {
        let first = isLoop ? 1 : 0
        guard !offsets.isEmpty, offsets.allSatisfy({ (first..<points.count).contains($0) }), (first...points.count).contains(destination) else { return }
        edit { next in
            let moved = offsets.map { next.points[$0] }
            next.points = next.points.enumerated().filter { !offsets.contains($0.offset) }.map(\.element)
            next.points.insert(contentsOf: moved, at: destination - offsets.filter { $0 < destination }.count)
        }
    }
    /// "Back to start": the finish becomes the last stop, and the route returns to the start.
    public func closeLoop() {
        guard hasRoute, !isLoop else { return }
        edit { $0.loop = true; $0.points[$0.points.count - 1].kind = .visit }
    }

    /// A signed route as a plan from its own start, named after the route: the shaping points in their order, each with
    /// its turnaround, and the Balanced preset that the catalog plan reproduces. A loop drops its closing point.
    public func planSignedRoute(_ plan: RoutePlan, loop: Bool, name: String, startName: String?, finishName: String?) {
        let closed = loop && plan.points.count > 2 && plan.points.last == plan.points.first
        let coordinates = closed ? Array(plan.points.dropLast()) : plan.points
        guard coordinates.count > 1 else { return }
        let turnarounds = Set(plan.turnarounds)
        edit { next in
            next.points = coordinates.enumerated().map { index, coordinate in
                let endpoint = index == 0 || (!loop && index == coordinates.count - 1)
                let label = index == 0 ? startName ?? Self.startName : endpoint ? finishName ?? "Finish" : Self.shapeName
                var point = PlannerPreviewPoint(place: .init(id: UUID().uuidString, name: label, coordinate: coordinate),
                                                kind: endpoint ? .visit : .shape)
                point.turnaround = turnarounds.contains(index)
                return point
            }
            next.markers = []; next.overnightPointID = nil; next.loop = loop; next.name = name; next.preset = .balanced
        }
    }

    /// Day ends follow the point order, so a loop with an overnight stop keeps its start.
    public var canMoveLoopStart: Bool { isLoop && overnightPointID == nil }

    /// Makes the point `id` the start of the loop. The points keep their order around the loop.
    public func startLoop(at id: String) { edit { Self.startLoop(&$0, at: id) } }

    // The old start becomes a stop when it has a name of its own, and a shaping point otherwise.
    private static func startLoop(_ state: inout State, at id: String) {
        guard state.loop, state.overnightPointID == nil,
              let index = state.points.firstIndex(where: { $0.id == id }), index > 0 else { return }
        var old = state.points[0]
        old.kind = [startName, shapeName, mapPointName].contains(old.place.name) ? .shape : .visit
        state.points = Array(state.points[index...]) + [old] + state.points[1..<index]
    }

    public func undo() {
        guard let previous = past.popLast() else { return }
        let key = routingKey
        future.append(state); state = previous
        invalidateRoute(from: key)
    }
    public func redo() {
        guard let next = future.popLast() else { return }
        let key = routingKey
        past.append(state); state = next
        invalidateRoute(from: key)
    }
    public func apply(_ action: PlannerPreviewAction) {
        switch action {
        case .createSample: loadSample()
        case .reverse:
            guard hasRoute else { return }
            edit { $0.points = $0.loop ? Array($0.points.prefix(1) + $0.points.dropFirst().reversed()) : $0.points.reversed() }
        case .splitDays:
            guard hasRoute else { return }
            setOvernight(mapPlaces.first { $0.kind == .camping })
        }
    }
    public func actionSummary(_ action: PlannerPreviewAction) -> String {
        switch action {
        case .createSample: "Freiburg to Titisee with online gravel routing."
        case .reverse: isLoop ? "Go around the loop the other way. Your stops reverse too."
            : "Start at \(finish?.name ?? "the finish") and go to \(start?.name ?? "the start"). Your stops reverse too."
        case .splitDays: "End day 1 at a campsite in the search results. Continue to \(finish?.name ?? "the finish") on day 2."
        }
    }

    /// Deliberately small example vocabulary; unsupported input never claims to be understood.
    public func lookup(_ query: String) -> PlannerPreviewQueryResult {
        let q = query.folding(options: [.diacriticInsensitive, .caseInsensitive], locale: .current)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        func result(_ title: String, _ explanation: String, places: [PlannerPreviewPlace] = [],
                    action: PlannerPreviewAction? = nil) -> PlannerPreviewQueryResult {
            .init(title: title, explanation: explanation, places: places, action: action)
        }
        if q.isEmpty { return result("Where would you like to go?", "Try Freiburg, Titisee, cafés or water.") }
        if q.contains("reverse") {
            return result(hasRoute ? "Go the other way" : "Create a route first", "Review the change before you apply it.", action: hasRoute ? .reverse : nil)
        }
        if q.contains("two day") || q.contains("2 day") || q.contains("split") {
            return result(hasRoute ? "Make it two days" : "Create a route first", "Find a campsite, then set it as your overnight stop.", action: hasRoute && mapPlaces.contains(where: { $0.kind == .camping }) ? .splitDays : nil)
        }
        if (q.contains("ride") || q.contains("route") || q.contains(" to ")) && (q.contains("titisee") || q.contains("freiburg")) {
            return result("Freiburg → Titisee", "An online gravel route. Review it before you apply it.", action: .createSample)
        }
        let kind: PlannerPreviewPlace.Kind? = q.contains("cafe") || q.contains("coffee") ? .cafe
            : q.contains("water") || q.contains("fountain") ? .water
            : q.contains("camp") || q.contains("sleep") ? .camping
            : q.contains("shop") || q.contains("supermarket") || q.contains("grocer") ? .shop : nil
        if let kind {
            return result(kind.title, "", places: mapPlaces.filter { $0.kind == kind })
        }
        let matches = mapPlaces.filter { $0.name.lowercased().contains(q) }
        return result(matches.isEmpty ? "No places found" : "Places", matches.isEmpty
                      ? "Try Freiburg, Titisee, cafés, water, reverse or two days. Search for a place to add to your route."
                      : "Choose a place to set an endpoint or add a stop.", places: matches)
    }

    private func edit(_ change: (inout State) -> Void) {
        var next = state; change(&next)
        // A loop needs a point to ride to before it returns.
        if next.points.count < 2 { next.loop = false }
        let stops = next.points.dropFirst().prefix(max(0, next.stopEnd - 1))
        if !stops.contains(where: { $0.id == next.overnightPointID && $0.kind == .visit }) {
            next.overnightPointID = nil
        }
        guard next != state else { return }
        let key = routingKey
        past.append(state); state = next; future.removeAll()
        invalidateRoute(from: key)
    }
    static let startName = "Start", shapeName = "Shaping point", mapPointName = "Map point"
    private static func newPoint(_ place: PlannerPreviewPlace, kind: PlannerPreviewPointKind = .visit, in state: State) -> PlannerPreviewPoint {
        let id = (state.points + state.markers).contains { $0.id == place.id } ? UUID().uuidString : place.id
        return .init(place: place, kind: kind, id: id)
    }
    private static var sampleState: State { State(points: [
        .init(place: .init(id: "freiburg", name: "Freiburg", coordinate: .init(latitude: 47.997922, longitude: 7.842534))),
        .init(place: .init(id: "titisee", name: "Titisee", coordinate: .init(latitude: 47.905528, longitude: 8.153371)))
    ]) }
    #if DEBUG
    public static let sampleMapPlaces: [PlannerPreviewPlace] = [
        .init(id: "freiburg", name: "Freiburg", coordinate: .init(latitude: 47.997922, longitude: 7.842534)),
        .init(id: "titisee", name: "Titisee", coordinate: .init(latitude: 47.905528, longitude: 8.153371), alongRouteMeters: 30_524),
        .init(id: "cafe", name: "Valley café", coordinate: .init(latitude: 47.9592, longitude: 7.9935), kind: .cafe,
              alongRouteMeters: 14_000, offRouteMeters: 80, hours: "Tu-Su 09:00-18:00", note: "Outdoor seating"),
        .init(id: "water", name: "Village fountain", coordinate: .init(latitude: 47.9451, longitude: 8.0385), kind: .water,
              alongRouteMeters: 19_000, offRouteMeters: 20, note: "Drinking water"),
        .init(id: "camping", name: "Forest campsite", coordinate: .init(latitude: 47.9288, longitude: 8.0798), kind: .camping,
              alongRouteMeters: 23_000, offRouteMeters: 90, hours: "08:00-12:00,16:00-20:00", note: "Tent pitches, showers"),
        .init(id: "cafe-orchard", name: "Orchard café", coordinate: .init(latitude: 47.9851, longitude: 7.9122), kind: .cafe,
              alongRouteMeters: 6_500, offRouteMeters: 50, hours: "We-Su 10:00-17:00"),
        .init(id: "shop-village", name: "Village food shop", coordinate: .init(latitude: 47.9660, longitude: 7.9650), kind: .shop,
              alongRouteMeters: 11_000, offRouteMeters: 100, hours: "Mo-Sa 07:30-19:00"),
        .init(id: "shop-lake", name: "Lakeside food shop", coordinate: .init(latitude: 47.9070, longitude: 8.1490), kind: .shop,
              alongRouteMeters: 29_800, offRouteMeters: 200, hours: "Mo-Sa 08:00-20:00; Su 09:00-13:00"),
        .init(id: "water-valley", name: "Valley water tap", coordinate: .init(latitude: 47.9790, longitude: 7.9300), kind: .water,
              alongRouteMeters: 8_300, offRouteMeters: 40, note: "Drinking water, seasonal"),
        .init(id: "cafe-hillside", name: "Hillside café", coordinate: .init(latitude: 47.9164, longitude: 8.1178), kind: .cafe,
              alongRouteMeters: 27_000, offRouteMeters: 160, hours: "08:00-19:00"),
    ]

    #endif
}
