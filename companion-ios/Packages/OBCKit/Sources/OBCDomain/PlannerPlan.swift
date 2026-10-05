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
    public var mode: Mode?
    /// The signed route the plan was made from.
    public var name: String?
    /// The planner activity: `road`, `gravel`, `mtb`, `touring` or `hiking`.
    public var bike: String?
    /// The preset title, such as `Balanced`.
    public var preset: String?
    /// Only `true` is written: the start also ends the route.
    public var loop: Bool?
    /// The IDs of all route points between the start and the finish, in ride order.
    public var routeOrder: [String]

    public init(points: [PlanPoint], mode: Mode? = nil, name: String? = nil, bike: String? = nil, preset: String? = nil,
                loop: Bool = false, routeOrder: [String]) {
        self.points = points
        days = points.filter { $0.kind == .night }.count + 1
        target = Double(days)
        self.mode = mode; self.name = name; self.bike = bike; self.preset = preset
        self.loop = loop ? true : nil
        self.routeOrder = routeOrder
    }

    public var isLoop: Bool { loop == true }

    /// The start, the points of ``routeOrder``, then the finish. A loop does not repeat its start.
    public var routePoints: [PlanPoint] {
        points.filter { $0.kind == .start } + routeOrder.compactMap { id in points.first { $0.id == id } }
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
    /// A line about the place, such as a route file's waypoint description.
    public var note: String?

    public init(id: String, label: String, coordinate: Coordinate, kind: Kind, night: Int? = nil,
                placeKind: String? = nil, leg: Leg? = nil, drawn: [RoutePoint]? = nil, turnaround: Bool = false,
                note: String? = nil) {
        self.id = id; self.label = label; self.coordinate = coordinate; self.kind = kind
        self.night = night; self.placeKind = placeKind; self.leg = leg; self.drawn = drawn
        self.turnaround = turnaround ? true : nil
        self.note = note
    }

    private enum CodingKeys: String, CodingKey {
        case id, label, coordinate, kind, night, placeKind, leg, drawn, turnaround, note
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
        note = try c.decodeIfPresent(String.self, forKey: .note)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(label, forKey: .label)
        try c.encode([coordinate.longitude, coordinate.latitude], forKey: .coordinate)
        try c.encode(kind, forKey: .kind)
        try c.encodeIfPresent(night, forKey: .night)
        try c.encodeIfPresent(placeKind, forKey: .placeKind)
        try c.encodeIfPresent(leg, forKey: .leg)
        try c.encodeIfPresent(drawn?.map { [$0.coordinate.longitude, $0.coordinate.latitude] + ($0.elevationMeters.map { [$0] } ?? []) },
                              forKey: .drawn)
        try c.encodeIfPresent(turnaround, forKey: .turnaround)
        try c.encodeIfPresent(note, forKey: .note)
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
            PlanPoint(id: "start", label: startName, coordinate: first, kind: .start),
            PlanPoint(id: "finish", label: finishName, coordinate: last, kind: .finish,
                      leg: .drawn, drawn: drawnLeg(from: first, along: line)),
        ] + markers(waypoints), mode: .route, routeOrder: [])
    }

    /// Route waypoints as markers.
    static func markers(_ waypoints: [Waypoint]) -> [PlanPoint] {
        waypoints.enumerated().map { index, waypoint in
            PlanPoint(id: "waypoint-\(index + 1)", label: waypoint.name, coordinate: waypoint.coordinate,
                      kind: .marker, placeKind: waypoint.category?.placeKind, note: waypoint.note)
        }
    }

    /// A trip kept as it is: one drawn leg per day, a night at each day end, and a transfer leg
    /// to a plain route point where the next day starts farther than ``Trip/transferMinMeters``
    /// from where a day ends. Nil for a trip of more than ``maxDays`` days.
    public static func keptLine(_ trip: Trip) -> PlannerPlan? {
        let days = trip.dayLines().map(\.points).filter { $0.count > 1 }
        guard let first = days.first?.first?.coordinate, days.count == trip.dayCount, days.count <= maxDays else { return nil }
        var points = [PlanPoint(id: "start", label: trip.startName ?? "Start", coordinate: first, kind: .start)]
        var here = first
        for (day, line) in days.enumerated() {
            if let start = line.first?.coordinate, start.distance(to: here) > Trip.transferMinMeters {
                points.append(PlanPoint(id: "day-\(day + 1)", label: trip.dayEnds[day - 1].resumeName ?? "Start of day \(day + 1)",
                                        coordinate: start, kind: .via, leg: .transfer))
                here = start
            }
            let end = line[line.count - 1].coordinate, isLast = day == days.count - 1
            points.append(PlanPoint(id: isLast ? "finish" : "night-\(day + 1)", label: trip.dayEnds[day].name ?? (isLast ? "Finish" : "End of day \(day + 1)"),
                                    coordinate: end, kind: isLast ? .finish : .night, night: isLast ? nil : day + 1,
                                    leg: .drawn, drawn: drawnLeg(from: here, along: line)))
            here = end
        }
        let order = points.dropFirst().dropLast().map(\.id)
        points += trip.waypoints.enumerated().map { index, stop in
            PlanPoint(id: "waypoint-\(index + 1)", label: stop.name, coordinate: stop.coordinate, kind: .marker)
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

// MARK: Imports

extension PlannerPlan {
    /// A line planned on roads: its start, a shaping point for each inner point, its finish, and the
    /// waypoints as markers.
    public static func shaped(_ points: [Coordinate], turnarounds: [Int], waypoints: [Waypoint] = []) -> PlannerPlan? {
        guard points.count > 1 else { return nil }
        let last = points.count - 1
        let route = points.indices.map { index in
            PlanPoint(id: index == 0 ? "start" : index == last ? "finish" : UUID().uuidString,
                      label: index == 0 ? "Start" : index == last ? "Finish" : "Shaping point", coordinate: points[index],
                      kind: index == 0 ? .start : index == last ? .finish : .via, turnaround: turnarounds.contains(index))
        }
        return PlannerPlan(points: route + markers(waypoints), mode: .route, routeOrder: route.dropFirst().dropLast().map(\.id))
    }

    /// The most days a plan has, as `specs/planner-plan.md` caps `days`.
    public static let maxDays = 14

    /// Route plans as the days of one trip, in ride order. Nil for more than ``maxDays`` days. Each finish but the last is a night. A day that
    /// starts farther than ``Trip/transferMinMeters`` from the night before starts with a transfer leg; a
    /// nearer day continues from that night.
    public static func trip(days: [PlannerPlan]) -> PlannerPlan? {
        guard (1...maxDays).contains(days.count) else { return nil }
        var points: [PlanPoint] = [], markers: [PlanPoint] = []
        for (day, plan) in days.enumerated() {
            var route = plan.routePoints
            guard route.count > 1 else { return nil }
            if let night = points.last {
                if route[0].coordinate.distance(to: night.coordinate) > Trip.transferMinMeters {
                    route[0].id = "day-\(day + 1)"; route[0].label = "Start of day \(day + 1)"
                    // A plain route point, not a stop: the day starts there.
                    route[0].kind = .via; route[0].leg = .transfer; route[0].drawn = nil
                } else {
                    route.removeFirst()
                }
            }
            if day < days.count - 1 {
                let end = route.count - 1
                route[end].id = "night-\(day + 1)"; route[end].label = "End of day \(day + 1)"
                route[end].kind = .night; route[end].night = day + 1
            }
            points += route
            markers += plan.markers.map { marker in
                var marker = marker
                marker.id = "d\(day + 1)-\(marker.id)"
                return marker
            }
        }
        return PlannerPlan(points: points + markers, mode: .trip, bike: days[0].bike,
                           routeOrder: points.dropFirst().dropLast().map(\.id))
    }
}
