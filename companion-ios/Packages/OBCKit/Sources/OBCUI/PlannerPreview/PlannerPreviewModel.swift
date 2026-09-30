#if DEBUG
import Foundation
import Observation
import OBCDomain

public struct PlannerPreviewPlace: Identifiable, Equatable, Sendable {
    public enum Kind: String, Sendable {
        case town, cafe, water, camping, shop
        public var title: String {
            switch self { case .town: "Place"; case .cafe: "Café"; case .water: "Water"; case .camping: "Camping"; case .shop: "Food shop" }
        }
        public var symbol: String {
            switch self { case .town: "mappin"; case .cafe: "cup.and.saucer.fill"; case .water: "drop.fill"; case .camping: "tent.fill"; case .shop: "cart.fill" }
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

/// A reversible interaction prototype. Route edits use the fixed sample line and straight connectors.
@MainActor @Observable
public final class PlannerPreviewModel {
    private struct State: Equatable {
        var points: [PlannerPreviewPoint] = []
        var markers: [PlannerPreviewPoint] = []
        var bike: BikeType = .gravel
        var preset: PlannerPreviewPreset = .balanced
        var overnightPointID: String?
    }
    private var state = State()
    private var past: [State] = []
    private var future: [State] = []

    public init(sample: Bool = false) {
        if sample { state = Self.sampleState }
    }

    public var points: [PlannerPreviewPoint] { state.points }
    public var markers: [PlannerPreviewPoint] { state.markers }
    public var start: PlannerPreviewPlace? { points.first?.place }
    public var finish: PlannerPreviewPlace? { points.count > 1 ? points.last?.place : nil }
    public var bike: BikeType { state.bike }
    public var preset: PlannerPreviewPreset { state.preset }
    public var overnightPointID: String? { state.overnightPointID }
    public var overnight: PlannerPreviewPlace? { points.first { $0.id == overnightPointID }?.place }
    public var hasRoute: Bool { points.count > 1 }
    public var canUndo: Bool { !past.isEmpty }
    public var canRedo: Bool { !future.isEmpty }
    public var dayCount: Int { overnight == nil ? 1 : 2 }
    public var routeTitle: String {
        guard let start, let finish else { return "New route" }
        return "\(start.name) → \(finish.name)"
    }
    public var routePoints: [RoutePoint] { sampledRoute.samples }
    public var routeLine: MeasuredLine { MeasuredLine(routePoints: routePoints) }
    public var geometry: [Coordinate] { routePoints.map(\.coordinate) }
    public var pointDistances: [String: Double] {
        let data = sampledRoute, line = MeasuredLine(routePoints: data.samples)
        return data.indices.mapValues { line.vertices[$0].distance }
    }
    public var stats: PlannerPreviewStats {
        let line = routeLine
        return stats(from: 0, to: line.length, on: line)
    }
    public var dayStats: [PlannerPreviewStats] {
        let line = routeLine
        guard let id = overnightPointID, let split = pointDistances[id] else { return [stats] }
        return [stats(from: 0, to: split, on: line), stats(from: split, to: line.length, on: line)]
    }

    public func exportRoute(name: String) -> ImportedRoute {
        let distances = pointDistances
        let located = points.dropFirst().dropLast().filter { $0.kind == .visit }.map { ($0.place, distances[$0.id] ?? 0) }
        let waypoints = located.enumerated().map { index, entry in
            let category: WaypointCategory? = switch entry.0.kind {
            case .water: .water
            case .camping: .campsite
            case .shop: .resupply
            case .town, .cafe: nil
            }
            return Waypoint(index: index, name: entry.0.name, note: "Illustrative preview stop",
                            distanceAlongMeters: entry.1, coordinate: entry.0.coordinate, category: category)
        }
        let title = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return ImportedRoute(name: "Preview · \(title.isEmpty ? routeTitle : title)",
                             creator: "OpenBikeComputer Planner Preview", points: routePoints, waypoints: waypoints)
    }

    public func newRoute() { edit { $0 = State() } }
    public func loadSample() { edit { $0 = Self.sampleState } }
    public func setStart(_ place: PlannerPreviewPlace) {
        edit { if $0.points.isEmpty { $0.points.append(Self.newPoint(place, in: $0)) } else { $0.points[0].place = place } }
    }
    public func setFinish(_ place: PlannerPreviewPlace) {
        edit {
            if $0.points.count < 2 {
                $0.points.append(Self.newPoint(place, in: $0))
            }
            else { $0.points[$0.points.count - 1].place = place }
        }
    }
    public func setBike(_ bike: BikeType) { edit { $0.bike = bike } }
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
                next.points.insert(point, at: next.points.count - 1)
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
            else { $0.points.insert(point, at: $0.points.count > 1 ? $0.points.count - 1 : $0.points.count) }
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
                next.points.insert(point, at: next.points.count > 1 ? next.points.count - 1 : next.points.count)
            }
        }
    }
    public func movePoint(fromOffsets offsets: IndexSet, toOffset destination: Int) {
        guard !offsets.isEmpty, offsets.allSatisfy({ points.indices.contains($0) }), (0...points.count).contains(destination) else { return }
        edit { next in
            let moved = offsets.map { next.points[$0] }
            next.points = next.points.enumerated().filter { !offsets.contains($0.offset) }.map(\.element)
            next.points.insert(contentsOf: moved, at: destination - offsets.filter { $0 < destination }.count)
        }
    }
    public func undo() {
        guard let previous = past.popLast() else { return }
        future.append(state); state = previous
    }
    public func redo() {
        guard let next = future.popLast() else { return }
        past.append(state); state = next
    }
    public func apply(_ action: PlannerPreviewAction) {
        switch action {
        case .createSample: loadSample()
        case .reverse:
            guard hasRoute else { return }
            edit { $0.points.reverse() }
        case .splitDays:
            guard hasRoute else { return }
            setOvernight(Self.sampleMapPlaces.first { $0.kind == .camping })
        }
    }
    public func actionSummary(_ action: PlannerPreviewAction) -> String {
        switch action {
        case .createSample: "Freiburg to Titisee on the sample gravel route."
        case .reverse: "Start at \(finish?.name ?? "the finish") and ride to \(start?.name ?? "the start"). Your stops reverse too."
        case .splitDays: "End day 1 at the sample campsite. Continue to \(finish?.name ?? "the finish") on day 2."
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
        if q.isEmpty { return result("Where would you like to ride?", "Try Freiburg, Titisee, cafés or water.") }
        if q.contains("reverse") {
            return result(hasRoute ? "Ride the other way" : "Create a route first", "Review the change before you apply it.", action: hasRoute ? .reverse : nil)
        }
        if q.contains("two day") || q.contains("2 day") || q.contains("split") {
            return result(hasRoute ? "Make it two days" : "Create a route first", "This preview uses one sample campsite.", action: hasRoute ? .splitDays : nil)
        }
        if (q.contains("ride") || q.contains("route") || q.contains(" to ")) && (q.contains("titisee") || q.contains("freiburg")) {
            return result("Freiburg → Titisee", "A fixed sample gravel route. Review it before you apply it.", action: .createSample)
        }
        let kind: PlannerPreviewPlace.Kind? = q.contains("cafe") || q.contains("coffee") ? .cafe
            : q.contains("water") || q.contains("fountain") ? .water
            : q.contains("camp") || q.contains("sleep") ? .camping
            : q.contains("shop") || q.contains("supermarket") || q.contains("grocer") ? .shop : nil
        if let kind {
            return result(kind.title, "", places: Self.sampleMapPlaces.filter { $0.kind == kind })
        }
        let matches = Self.sampleMapPlaces.filter { $0.name.lowercased().contains(q) }
        return result(matches.isEmpty ? "No preview match" : "Places", matches.isEmpty
                      ? "Try Freiburg, Titisee, cafés, water, reverse or two days. This preview supports these examples."
                      : "Choose a place to set an endpoint or add a stop.", places: matches)
    }

    private func edit(_ change: (inout State) -> Void) {
        var next = state; change(&next)
        if !next.points.dropFirst().dropLast().contains(where: { $0.id == next.overnightPointID && $0.kind == .visit }) {
            next.overnightPointID = nil
        }
        guard next != state else { return }
        past.append(state); state = next; future.removeAll()
    }
    private static func newPoint(_ place: PlannerPreviewPlace, kind: PlannerPreviewPointKind = .visit, in state: State) -> PlannerPreviewPoint {
        let id = (state.points + state.markers).contains { $0.id == place.id } ? UUID().uuidString : place.id
        return .init(place: place, kind: kind, id: id)
    }
    private static var sampleState: State { State(points: sampleMapPlaces.prefix(2).map { .init(place: $0) }) }
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

    private var sampledRoute: (samples: [RoutePoint], indices: [String: Int]) {
        guard hasRoute else { return ([], [:]) }
        let base = zip(PlannerPreviewFixture.coordinates, PlannerPreviewFixture.elevation).map {
            RoutePoint(coordinate: $0.0, elevationMeters: $0.1)
        }
        func closest(_ place: PlannerPreviewPlace) -> Int {
            base.indices.min { base[$0].coordinate.distance(to: place.coordinate) < base[$1].coordinate.distance(to: place.coordinate) }!
        }
        var samples: [RoutePoint] = []
        var pointIndices: [String: Int] = [:]
        for (a, b) in zip(points, points.dropFirst()) {
            let from = closest(a.place), to = closest(b.place)
            if samples.isEmpty { pointIndices[a.id] = 0 }
            samples.append(RoutePoint(coordinate: a.place.coordinate, elevationMeters: base[from].elevationMeters))
            let indices = from <= to ? Array(from...to) : Array((to...from).reversed())
            samples.append(contentsOf: indices.map { base[$0] })
            samples.append(RoutePoint(coordinate: b.place.coordinate, elevationMeters: base[to].elevationMeters))
            pointIndices[b.id] = samples.count - 1
        }
        return (samples, pointIndices)
    }
    private func stats(from: Double, to: Double, on line: MeasuredLine) -> PlannerPreviewStats {
        guard !line.vertices.isEmpty else { return .init(distanceMeters: 0, ascentMeters: 0, seconds: 0) }
        let distance = max(0, to - from), ascent = line.climb(from: from, to: to)
        return .init(distanceMeters: distance, ascentMeters: ascent,
                     seconds: bike.ridingTime(distanceMeters: distance, ascentMeters: ascent))
    }
}
#endif
