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
    public let website: String?
    public let phone: String?
    public let description: String?
    public let detailsLoaded: Bool

    public init(id: String, name: String, coordinate: Coordinate, kind: Kind = .town,
                alongRouteMeters: Double = 0, offRouteMeters: Double = 0, hours: String? = nil, note: String? = nil,
                website: String? = nil, phone: String? = nil, description: String? = nil, detailsLoaded: Bool = false) {
        self.id = id; self.name = name; self.coordinate = coordinate; self.kind = kind
        self.alongRouteMeters = alongRouteMeters; self.offRouteMeters = offRouteMeters
        self.hours = hours; self.note = note
        self.website = website; self.phone = phone; self.description = description
        self.detailsLoaded = detailsLoaded
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

/// A leg that does not simply follow the router, or that keeps a drawn line to go back to.
public struct PlannerPreviewLeg: Equatable, Sendable {
    public var mode: PlanPoint.Leg
    /// The inner line of the leg, without its end points, with elevations where known.
    public var drawn: [RoutePoint]?
    public init(mode: PlanPoint.Leg, drawn: [RoutePoint]? = nil) { self.mode = mode; self.drawn = drawn }
}

/// A tap on the line of a leg: the IDs of the points the leg joins and the tapped point on its line.
public struct PlannerPreviewLegHit: Equatable, Sendable {
    public let from: String
    public let to: String
    public let coordinate: Coordinate
}

/// Editing intent is reversible. Geometry comes from the published route service.
@MainActor @Observable
public final class PlannerPreviewModel {
    struct LegID: Hashable { let from: String; let to: String }
    private struct State: Equatable {
        var points: [PlannerPreviewPoint] = []
        var markers: [PlannerPreviewPoint] = []
        var activity = RouteActivity.gravel
        var preset: PlannerPreviewPreset = .balanced
        /// The points where a day ends.
        var nights: Set<String> = []
        /// Only the legs that are not plainly routed. A leg joins two consecutive route points.
        var legs: [LegID: PlannerPreviewLeg] = [:]
        /// The finish is the start: the route returns to the first point, and there is no finish point.
        var loop = false
        /// The title of a plan from a signed route.
        var name: String?
        /// Where a new stop goes: before the finish, or at the end of a loop.
        var stopEnd: Int { loop || points.count < 2 ? points.count : points.count - 1 }
        /// The route points in ride order; a loop ends at its start again.
        var route: [PlannerPreviewPoint] { points + (loop ? points.prefix(1) : []) }
        var coordinates: [Coordinate] { route.map(\.place.coordinate) }
        var legIDs: [LegID] { zip(route, route.dropFirst()).map { LegID(from: $0.id, to: $1.id) } }
        /// Where day 1 ends. The overnight controls change this night and leave the others.
        var overnight: String? {
            get { points.first { nights.contains($0.id) }?.id }
            set {
                if let old = overnight { nights.remove(old) }
                if let newValue { nights.insert(newValue) }
            }
        }
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

    /// Opens `plan` without an undo step.
    public init(plan: PlannerPlan, service: any PlannerDataSource = PlannerService.shared) {
        self.service = service
        state = Self.state(plan)
    }

    private struct RoutingKey: Equatable {
        let coordinates: [Coordinate]
        let turnarounds: [Int]
        let activity: RouteActivity
        let preset: PlannerPreviewPreset
        /// One per leg; nil for a routed leg.
        let legs: [PlannerPreviewLeg?]
    }
    private var routingKey: RoutingKey {
        .init(coordinates: state.coordinates, turnarounds: state.turnarounds, activity: activity, preset: preset,
              legs: state.legIDs.map { state.legs[$0].flatMap { $0.mode == .routed ? nil : $0 } })
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
            let result = try await path(key, preference: preference, release: selected)
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

    /// Consecutive routed legs are one request, so a shaping point keeps its road direction. Every other
    /// leg is its line: the drawn line, or a straight one.
    private func path(_ key: RoutingKey, preference: RoutePreference, release: PlannerRelease) async throws -> PlannedPath {
        let nodes = key.coordinates
        var path = PathBuilder()
        var leg = 0
        while leg < key.legs.count {
            if let special = key.legs[leg] {
                path.append(from: nodes[leg], to: nodes[leg + 1], inner: special.mode == .drawn ? special.drawn ?? [] : [],
                            timed: special.mode != .transfer)
                leg += 1
                continue
            }
            var end = leg + 1
            while end < key.legs.count, key.legs[end] == nil { end += 1 }
            let turns = key.turnarounds.filter { $0 > leg && $0 < end }.map { $0 - leg }
            path.append(try await service.route(points: Array(nodes[leg...end]), turnarounds: turns, activity: key.activity,
                                                preference: preference, release: release))
            leg = end
        }
        return path.result
    }

    public func positionedPlace(_ place: PlannerPreviewPlace) -> PlannerPreviewPlace {
        guard routeLine.length > 0 else { return place }
        let projection = routeLine.projection(of: place.coordinate, near: routeLine.length / 2, window: routeLine.length)
        return .init(id: place.id, name: place.name, coordinate: place.coordinate, kind: place.kind,
                     alongRouteMeters: projection.distance, offRouteMeters: projection.error,
                     hours: place.hours, note: place.note, website: place.website, phone: place.phone, description: place.description, detailsLoaded: place.detailsLoaded)
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
                         hours: place.opening_hours, note: place.city.isEmpty ? nil : place.city,
                         website: place.website, phone: place.phone, description: place.description, detailsLoaded: true)
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
    /// The activities whose Balanced profile the release lists. All of them while the release is unknown.
    public private(set) var activities = RouteActivity.allCases
    public func loadActivities() async {
        guard let release, let profiles = try? await service.profiles(release: release) else { return }
        activities = RouteActivity.allCases.filter { profiles.contains(RoutePreference.balanced.profile(for: $0)) }
    }
    public var preset: PlannerPreviewPreset { state.preset }
    /// The points where a day ends, in ride order.
    public var nights: [PlannerPreviewPoint] { points.filter { state.nights.contains($0.id) } }
    /// Where day 1 ends.
    public var overnightPointID: String? { nights.first?.id }
    public var overnight: PlannerPreviewPlace? { nights.first?.place }
    public var hasRoute: Bool { points.count > 1 }
    public var canUndo: Bool { !past.isEmpty }
    /// The number of edits that undo can take back. It changes with every edit, undo and redo.
    public var undoDepth: Int { past.count }
    public var planName: String? { state.name }
    public var canRedo: Bool { !future.isEmpty }
    public var dayCount: Int { nights.count + 1 }
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
        guard let path, !state.nights.isEmpty else { return [stats] }
        // Each day ends at a night and the last at the line end, as (distance along, climb, elapsed seconds).
        let ends = points.indices.filter { state.nights.contains(points[$0].id) }.map { position in
            let index = path.pointIndices[position], split = routeLine.vertices[index].distance
            return (split, min(path.ascent, routeLine.climb(from: 0, to: split)), path.elapsed[index])
        } + [(routeLine.length, path.ascent, path.seconds)]
        let length = routeLine.length
        return zip([(0.0, 0.0, 0.0)] + ends, ends).map { from, to in
            .init(distanceMeters: length > 0 ? path.distance * (to.0 - from.0) / length : 0, ascentMeters: max(0, to.1 - from.1), seconds: max(0, to.2 - from.2))
        }
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

    /// The plan behind the line, in the `specs/planner-plan.md` shape. Nights take the IDs `night-N`.
    public func exportPlan() -> PlannerPlan {
        let route = state.route, nightIDs = nights.map(\.id)
        func id(_ point: PlannerPreviewPoint) -> String {
            if let night = nightIDs.firstIndex(of: point.id) { return "night-\(night + 1)" }
            return point.id.hasPrefix("night-") ? UUID().uuidString : point.id
        }
        let ids = Dictionary((points + markers).map { ($0.id, id($0)) }, uniquingKeysWith: { a, _ in a })
        let distances = pointDistances, length = routeLine.length
        // The leg into each point; in a loop the start's leg is the closing leg.
        let legs = Dictionary(state.legIDs.compactMap { leg in state.legs[leg].map { (leg.to, $0) } }, uniquingKeysWith: { a, _ in a })
        let planned = points.enumerated().map { index, point in
            let kind: PlanPoint.Kind = index == 0 ? .start : isEndpoint(point.id) ? .finish
                : nightIDs.contains(point.id) ? .night : point.kind == .shape ? .via : .waypoint
            let leg = legs[point.id]
            return PlanPoint(id: ids[point.id]!, label: point.place.name, coordinate: point.place.coordinate,
                             progress: length > 0 ? min(1, (distances[point.id] ?? 0) / length) : 0, kind: kind,
                             night: nightIDs.firstIndex(of: point.id).map { $0 + 1 },
                             placeKind: point.place.kind == .town ? nil : point.place.kind.rawValue,
                             leg: leg.flatMap { $0.mode == .routed ? nil : $0.mode }, drawn: leg?.drawn, turnaround: point.turnaround)
        } + markers.map { PlanPoint(id: ids[$0.id]!, label: $0.place.name, coordinate: $0.place.coordinate, progress: 0, kind: .marker,
                                    placeKind: $0.place.kind == .town ? nil : $0.place.kind.rawValue) }
        return PlannerPlan(points: planned, mode: .route, name: state.name, bike: activity.rawValue, preset: preset.title,
                           loop: isLoop, routeOrder: route.dropFirst().dropLast().map { ids[$0.id]! })
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
            guard let place else { next.nights = []; return }
            if let index = next.points.firstIndex(where: { $0.place.id == place.id }) {
                next.points[index].kind = .visit
                next.overnight = next.points[index].id
            } else {
                let marker = next.markers.first { $0.place.id == place.id }
                let point = marker.map { PlannerPreviewPoint(place: place, id: $0.id) } ?? Self.newPoint(place, in: next)
                next.markers.removeAll { $0.id == point.id }
                next.points.insert(point, at: next.stopEnd)
                next.overnight = point.id
            }
        }
    }
    public func setOvernightPoint(id: String?) {
        edit { next in
            next.overnight = id
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

    // MARK: Legs

    /// The leg whose line passes nearest `coordinate`, within `tolerance` metres. Only a leg with a drawn
    /// line answers: the other legs keep the map point actions.
    public func leg(near coordinate: Coordinate, within tolerance: Double) -> PlannerPreviewLegHit? {
        guard let path, path.pointIndices.count == state.legIDs.count + 1 else { return nil }
        var best: (hit: PlannerPreviewLegHit, error: Double)?
        for (index, id) in state.legIDs.enumerated() where state.legs[id]?.drawn != nil {
            let line = MeasuredLine(routePoints: Array(path.points[path.pointIndices[index]...path.pointIndices[index + 1]]))
            let projection = line.projection(of: coordinate, near: line.length / 2, window: line.length)
            guard projection.error <= tolerance, projection.error < best?.error ?? .infinity else { continue }
            best = (.init(from: id.from, to: id.to, coordinate: line.coordinate(at: projection.distance)), projection.error)
        }
        return best?.hit
    }

    public func leg(_ hit: PlannerPreviewLegHit) -> PlannerPreviewLeg {
        state.legs[LegID(from: hit.from, to: hit.to)] ?? .init(mode: .routed)
    }

    /// A leg with a drawn line can follow the router and go back to the line.
    public func setLegMode(_ hit: PlannerPreviewLegHit, to mode: PlanPoint.Leg) {
        let id = LegID(from: hit.from, to: hit.to)
        guard let leg = state.legs[id], mode != .drawn || leg.drawn != nil else { return }
        edit { $0.legs[id]?.mode = mode }
    }

    /// Adds a shaping point on the line of a leg. A drawn line splits there and does not change.
    public func addPoint(on hit: PlannerPreviewLegHit) {
        guard let from = state.points.first(where: { $0.id == hit.from }), let to = state.points.firstIndex(where: { $0.id == hit.to }) else { return }
        let id = LegID(from: hit.from, to: hit.to)
        var coordinate = hit.coordinate, halves: ([RoutePoint], [RoutePoint])?
        if let leg = state.legs[id], leg.mode == .drawn, let drawn = leg.drawn {
            let full = [RoutePoint(coordinate: from.place.coordinate)] + drawn + [RoutePoint(coordinate: state.points[to].place.coordinate)]
            let line = MeasuredLine(routePoints: full)
            let distance = line.project(hit.coordinate, near: line.length / 2, window: line.length)
            let segment = min(line.index(at: distance), full.count - 2)
            coordinate = line.coordinate(at: distance)
            // Both halves keep the height at the split, so the profile does not change either.
            let split = RoutePoint(coordinate: coordinate, elevationMeters: line.hasElevation ? line.elevation(at: distance) : nil)
            halves = (Array(full[1..<(segment + 1)]) + [split], [split] + full[(segment + 1)..<(full.count - 1)])
        }
        let point = PlannerPreviewPoint(place: .init(id: UUID().uuidString, name: Self.shapeName, coordinate: coordinate), kind: .shape)
        edit { next in
            // The closing leg of a loop ends at the start; its new point goes last.
            next.points.insert(point, at: to == 0 ? next.points.count : to)
            next.legs[id] = nil
            if let halves {
                next.legs[LegID(from: id.from, to: point.id)] = .init(mode: .drawn, drawn: halves.0)
                next.legs[LegID(from: point.id, to: id.to)] = .init(mode: .drawn, drawn: halves.1)
            }
        }
    }

    /// The editor state of a plan. Pass and detour points have no kind of their own here: a pass is a
    /// shaping point and a detour a stop.
    private static func state(_ plan: PlannerPlan) -> State {
        let route = plan.routePoints
        var state = State()
        state.points = route.map { planned in
            let kind = PlannerPreviewPlace.Kind(rawValue: planned.placeKind ?? "") ?? planned.placeKind.map(NativePlaceKind.kind(for:)) ?? .town
            var point = PlannerPreviewPoint(place: .init(id: planned.id, name: planned.label, coordinate: planned.coordinate, kind: kind),
                                            kind: [.via, .pass].contains(planned.kind) ? .shape : .visit)
            point.turnaround = planned.turnaround == true
            return point
        }
        state.markers = plan.markers.map {
            .init(place: .init(id: $0.id, name: $0.label, coordinate: $0.coordinate,
                               kind: PlannerPreviewPlace.Kind(rawValue: $0.placeKind ?? "") ?? .town), kind: .marker)
        }
        state.nights = Set(route.filter { $0.kind == .night }.map(\.id))
        state.loop = plan.isLoop && route.count > 1
        state.name = plan.name
        state.activity = plan.bike.flatMap(RouteActivity.init(rawValue:)) ?? .gravel
        state.preset = PlannerPreviewPreset.allCases.first { $0.title == plan.preset } ?? .balanced
        let ends = route + (state.loop ? route.prefix(1) : [])
        for (from, to) in zip(ends, ends.dropFirst()) where (to.leg ?? .routed) != .routed || to.drawn != nil {
            state.legs[LegID(from: from.id, to: to.id)] = .init(mode: to.leg ?? .routed, drawn: to.drawn)
        }
        return state
    }

    /// A signed route as a plan from its own start, named after the route: the shaping points in their order, each with
    /// its turnaround, and the Balanced preset that the catalog plan reproduces. A loop drops its closing point.
    public func planSignedRoute(_ plan: RoutePlan, loop: Bool, name: String, startName: String?, finishName: String?) {
        let closed = loop && plan.points.count > 2 && plan.points.last == plan.points.first
        let coordinates = closed ? Array(plan.points.dropLast()) : plan.points
        guard coordinates.count > 1, plan.requestPoints(loop: loop).count <= RoutePlan.maxPoints else { return }
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
            next.markers = []; next.nights = []; next.legs = [:]; next.loop = loop; next.name = name; next.preset = .balanced
        }
    }

    /// Day ends follow the point order, so a loop with an overnight stop keeps its start.
    public var canMoveLoopStart: Bool { isLoop && overnightPointID == nil }

    /// Makes the point `id` the start of the loop. The points keep their order around the loop.
    public func startLoop(at id: String) { edit { Self.startLoop(&$0, at: id) } }

    // The old start becomes a stop when it has a name of its own, and a shaping point otherwise.
    private static func startLoop(_ state: inout State, at id: String) {
        guard state.loop, state.nights.isEmpty,
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
            edit {
                $0.points = $0.loop ? Array($0.points.prefix(1) + $0.points.dropFirst().reversed()) : $0.points.reversed()
                $0.legs = Dictionary(uniqueKeysWithValues: $0.legs.map { id, leg in
                    (LegID(from: id.to, to: id.from), PlannerPreviewLeg(mode: leg.mode, drawn: leg.drawn?.reversed()))
                })
            }
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
        // A plan with fewer than two points is no longer the signed route it was named after.
        if next.points.count < 2 { next.loop = false; next.name = nil }
        let stops = next.points.dropFirst().prefix(max(0, next.stopEnd - 1))
        next.nights = next.nights.filter { id in stops.contains { $0.id == id && $0.kind == .visit } }
        // A leg keeps its line only while it joins the same two points at the same places.
        let moved = Set(next.points.filter { point in
            state.points.contains { $0.id == point.id && $0.place.coordinate != point.place.coordinate }
        }.map(\.id))
        let joined = Set(next.legIDs)
        next.legs = next.legs.filter { joined.contains($0.key) && !moved.contains($0.key.from) && !moved.contains($0.key.to) }
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

/// Joins routed runs and drawn or straight legs into one path. A piece that starts away from the
/// end of the path joins it with a straight line.
private struct PathBuilder {
    /// The speed of a line the router did not plan, as the web planner assumes.
    static let lineSpeed = 15 / 3.6
    private var points: [RoutePoint] = [], elapsed: [Double] = [], indices: [Int] = []
    private var distance = 0.0, ascent = 0.0, seconds = 0.0

    var result: PlannedPath {
        PlannedPath(points: points, distance: distance, ascent: ascent, seconds: seconds, pointIndices: indices, elapsed: elapsed)
    }

    mutating func append(_ run: PlannedPath) {
        let start = join(run.points, elapsed: run.elapsed)
        indices += run.pointIndices.dropFirst(indices.isEmpty ? 0 : 1).map { start + $0 }
        distance += run.distance; ascent += run.ascent; seconds += run.seconds
    }

    /// A drawn or straight leg. Its end points take the nearest known elevation, so the leg adds no
    /// unknown elevation and a straight or transfer leg no climb. `timed` false is a transfer: no riding time.
    mutating func append(from: Coordinate, to: Coordinate, inner: [RoutePoint], timed: Bool) {
        let head = inner.first?.elevationMeters ?? points.last?.elevationMeters
        let tail = inner.last?.elevationMeters ?? head
        let line = [RoutePoint(coordinate: from, elevationMeters: head)] + inner + [RoutePoint(coordinate: to, elevationMeters: tail)]
        let measured = MeasuredLine(routePoints: line)
        let times = measured.vertices.map { timed ? $0.distance / Self.lineSpeed : 0 }
        let start = join(line, elapsed: times)
        if indices.isEmpty { indices.append(start) }
        indices.append(start + line.count - 1)
        distance += measured.length; ascent += measured.climb(from: 0, to: measured.length); seconds += times.last ?? 0
    }

    /// Returns the index of the piece's first point in the path.
    private mutating func join(_ piece: [RoutePoint], elapsed times: [Double]) -> Int {
        let repeats = points.last?.coordinate == piece.first?.coordinate
        if !repeats, let last = points.last, let first = piece.first {
            let gap = last.coordinate.routeDistance(to: first.coordinate)
            distance += gap; seconds += gap / Self.lineSpeed
        }
        let start = points.count - (repeats ? 1 : 0), base = seconds
        for index in piece.indices.dropFirst(repeats ? 1 : 0) {
            points.append(piece[index]); elapsed.append(base + times[index])
        }
        return start
    }
}
