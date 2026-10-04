import Foundation

/// The plan object of `specs/planner-plan.md`: the fields iOS uses. The JSON is that contract, so
/// the type is `Codable` itself. Required web-only fields keep the values the web `emptyTrip()` writes.
public struct PlannerPlan: Codable, Equatable, Sendable {
    public enum Mode: String, Codable, Sendable { case route, trip }

    /// Route points and markers. ``routePoints`` gives the ride order.
    public var points: [PlanPoint]
    public var days: Int
    public var budget = "days"
    public var target: Double
    public var limit = 50.0
    public var variant = "valley"
    public var mode: Mode?
    /// The signed route the plan was made from.
    public var name: String?
    /// The planner activity: `road`, `gravel`, `mtb`, `touring` or `hiking`.
    public var bike: String?
    /// The preset title, such as `Balanced`.
    public var preset: String?
    /// Only `true` is written: the start also ends the route.
    public var loop: Bool?
    /// The IDs of the route points between the start and the finish, in ride order.
    public var routeOrder: [String]?

    public init(points: [PlanPoint], mode: Mode? = nil, name: String? = nil, bike: String? = nil, preset: String? = nil,
                loop: Bool = false, routeOrder: [String]? = nil) {
        self.points = points
        days = points.filter { $0.kind == .night }.count + 1
        target = Double(days)
        self.mode = mode; self.name = name; self.bike = bike; self.preset = preset
        self.loop = loop ? true : nil
        self.routeOrder = routeOrder
    }

    public var isLoop: Bool { loop == true }

    /// The start, the middle points in route order, then the finish, as the web planner orders them.
    /// A loop does not repeat its start.
    public var routePoints: [PlanPoint] {
        let middle = points.filter { ![.start, .finish, .marker].contains($0.kind) }.sorted { $0.progress < $1.progress }
        let order = routeOrder ?? middle.map(\.id)
        let ranked = order.compactMap { id in middle.first { $0.id == id } }
        return points.filter { $0.kind == .start } + ranked + middle.filter { !order.contains($0.id) }
            + (isLoop ? [] : points.filter { $0.kind == .finish })
    }

    public var markers: [PlanPoint] { points.filter { $0.kind == .marker } }
}

public struct PlanPoint: Codable, Equatable, Sendable {
    public enum Kind: String, Codable, Sendable { case start, finish, pass, via, waypoint, detour, night, marker }
    /// How the leg that ends at this point runs. In a loop, the start's leg is the closing leg.
    public enum Leg: String, Codable, Sendable { case routed, straight, drawn, transfer }

    public var id: String
    public var label: String
    public var coordinate: Coordinate
    /// Position along the route, from 0 to 1.
    public var progress: Double
    public var kind: Kind
    /// The night number, from 1, of a `night` point.
    public var night: Int?
    public var placeKind: String?
    public var leg: Leg?
    /// The inner line of the leg, without its two end points, with elevations where known. It can
    /// stay on a routed leg, so the leg can go back to the line.
    public var drawn: [RoutePoint]?
    /// Only `true` is written.
    public var turnaround: Bool?

    public init(id: String, label: String, coordinate: Coordinate, progress: Double, kind: Kind, night: Int? = nil,
                placeKind: String? = nil, leg: Leg? = nil, drawn: [RoutePoint]? = nil, turnaround: Bool = false) {
        self.id = id; self.label = label; self.coordinate = coordinate; self.progress = progress; self.kind = kind
        self.night = night; self.placeKind = placeKind; self.leg = leg; self.drawn = drawn
        self.turnaround = turnaround ? true : nil
    }

    private enum CodingKeys: String, CodingKey {
        case id, label, coordinate, progress, kind, night, placeKind, leg, drawn, turnaround
    }

    // Coordinates are `[longitude, latitude]`; a drawn vertex can add its elevation in metres.
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        label = try c.decode(String.self, forKey: .label)
        let pair = try c.decode([Double].self, forKey: .coordinate)
        guard pair.count == 2 else {
            throw DecodingError.dataCorruptedError(forKey: .coordinate, in: c, debugDescription: "A coordinate is [longitude, latitude].")
        }
        coordinate = Coordinate(latitude: pair[1], longitude: pair[0])
        progress = try c.decode(Double.self, forKey: .progress)
        kind = try c.decode(Kind.self, forKey: .kind)
        night = try c.decodeIfPresent(Int.self, forKey: .night)
        placeKind = try c.decodeIfPresent(String.self, forKey: .placeKind)
        leg = try c.decodeIfPresent(Leg.self, forKey: .leg)
        drawn = try c.decodeIfPresent([[Double]].self, forKey: .drawn)?.map { vertex in
            guard vertex.count == 2 || vertex.count == 3 else {
                throw DecodingError.dataCorruptedError(forKey: .drawn, in: c, debugDescription: "A drawn vertex is [longitude, latitude, elevation?].")
            }
            return RoutePoint(coordinate: Coordinate(latitude: vertex[1], longitude: vertex[0]), elevationMeters: vertex.count == 3 ? vertex[2] : nil)
        }
        turnaround = try c.decodeIfPresent(Bool.self, forKey: .turnaround) == true ? true : nil
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(label, forKey: .label)
        try c.encode([coordinate.longitude, coordinate.latitude], forKey: .coordinate)
        try c.encode(progress, forKey: .progress)
        try c.encode(kind, forKey: .kind)
        try c.encodeIfPresent(night, forKey: .night)
        try c.encodeIfPresent(placeKind, forKey: .placeKind)
        try c.encodeIfPresent(leg, forKey: .leg)
        try c.encodeIfPresent(drawn?.map { [$0.coordinate.longitude, $0.coordinate.latitude] + ($0.elevationMeters.map { [$0] } ?? []) },
                              forKey: .drawn)
        try c.encodeIfPresent(turnaround, forKey: .turnaround)
    }
}

// MARK: Kept lines

extension PlannerPlan {
    /// A drawn leg follows its line within this distance: close enough to look exact on the map.
    public static let keptLineToleranceMeters = 3.0

    /// A line kept as it is: a start, a finish and one drawn leg between them. The waypoints are
    /// markers, so a waypoint off the line adds no detour.
    public static func keptLine(_ line: [RoutePoint], waypoints: [Waypoint] = [], startName: String = "Start",
                                finishName: String = "Finish") -> PlannerPlan? {
        guard let first = line.first?.coordinate, let last = line.last?.coordinate, line.count > 1 else { return nil }
        return PlannerPlan(points: [
            PlanPoint(id: "start", label: startName, coordinate: first, progress: 0, kind: .start),
            PlanPoint(id: "finish", label: finishName, coordinate: last, progress: 1, kind: .finish,
                      leg: .drawn, drawn: drawnLeg(from: first, along: line)),
        ] + waypoints.enumerated().map { index, waypoint in
            PlanPoint(id: "waypoint-\(index + 1)", label: waypoint.name, coordinate: waypoint.coordinate, progress: 0,
                      kind: .marker, placeKind: waypoint.category?.placeKind)
        }, mode: .route)
    }

    /// A trip kept as it is: one drawn leg per day, a night at each day end, and a transfer leg
    /// where the next day starts farther than ``Trip/transferMinMeters`` from where a day ends.
    public static func keptLine(_ trip: Trip) -> PlannerPlan? {
        let days = trip.dayLines().map(\.points).filter { $0.count > 1 }
        guard let first = days.first?.first?.coordinate, days.count == trip.dayCount else { return nil }
        var points = [PlanPoint(id: "start", label: trip.startName ?? "Start", coordinate: first, progress: 0, kind: .start)]
        var distance = 0.0
        var here = first
        for (day, line) in days.enumerated() {
            if let start = line.first?.coordinate, start.distance(to: here) > Trip.transferMinMeters {
                distance += start.distance(to: here)
                points.append(PlanPoint(id: "day-\(day + 1)", label: trip.dayEnds[day - 1].resumeName ?? "Start of day \(day + 1)",
                                        coordinate: start, progress: distance, kind: .waypoint, leg: .transfer))
                here = start
            }
            let end = line[line.count - 1].coordinate, isLast = day == days.count - 1
            let path = [here] + line.map(\.coordinate)
            distance += zip(path, path.dropFirst()).reduce(0) { $0 + $1.0.distance(to: $1.1) }
            points.append(PlanPoint(id: isLast ? "finish" : "night-\(day + 1)", label: trip.dayEnds[day].name ?? (isLast ? "Finish" : "End of day \(day + 1)"),
                                    coordinate: end, progress: distance, kind: isLast ? .finish : .night, night: isLast ? nil : day + 1,
                                    leg: .drawn, drawn: drawnLeg(from: here, along: line)))
            here = end
        }
        for index in points.indices { points[index].progress = distance > 0 ? points[index].progress / distance : 0 }
        let order = points.dropFirst().dropLast().map(\.id)
        points += trip.waypoints.enumerated().map { index, stop in
            PlanPoint(id: "waypoint-\(index + 1)", label: stop.name, coordinate: stop.coordinate, progress: 0, kind: .marker,
                      placeKind: stop.kind == .campsite ? WaypointCategory.campsite.placeKind
                        : stop.kind == .hotel ? WaypointCategory.accommodation.placeKind : nil)
        }
        return PlannerPlan(points: points, mode: .trip, routeOrder: order)
    }

    /// The inner line of a drawn leg from `start` along `line` to its last point. Only the
    /// coordinate and the elevation of a vertex stay. A plan point has no elevation, so an end with
    /// one stays in the line, where the leg reads it.
    static func drawnLeg(from start: Coordinate, along line: [RoutePoint]) -> [RoutePoint] {
        var kept = RideMapLine.simplifiedIndices(line.map(\.coordinate), toleranceMeters: keptLineToleranceMeters)
            .map { RoutePoint(coordinate: line[$0].coordinate, elevationMeters: line[$0].elevationMeters) }
        if kept.last?.elevationMeters == nil { kept.removeLast() }
        if kept.first?.coordinate == start, kept.first?.elevationMeters == nil { kept.removeFirst() }
        return kept
    }
}

extension WaypointCategory {
    /// The planner place kind of the category, as a plan point's `placeKind` keeps it.
    public var placeKind: String {
        switch self {
        case .water: "water"
        case .campsite: "camping"
        case .accommodation: "hotel"
        case .resupply: "shop"
        case .pharmacy: "pharmacy"
        case .bikeShop: "bike"
        }
    }

    public init?(placeKind: String) {
        guard let category = Self.allCases.first(where: { $0.placeKind == placeKind }) else { return nil }
        self = category
    }
}
