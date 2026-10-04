import Foundation

extension PlannerPlan {
    /// A label the planner or a kept line gives a point that has no place name of its own.
    public static func isPlaceholder(_ label: String) -> Bool {
        ["Start", "Finish", "Map point", "Shaping point"].contains(label)
            || label.hasPrefix("End of day ") || label.hasPrefix("Start of day ")
    }

    /// The plan with `day` as its new last day: the finish becomes a night, a transfer leg goes to
    /// the day's start when that is farther than ``Trip/transferMinMeters``, the day is one drawn
    /// leg, and its waypoints are markers. Nil for a loop, or when the plan already has
    /// ``maxDays`` days.
    public func appendingDay(_ day: [RoutePoint], name: String?, waypoints: [Waypoint] = []) -> PlannerPlan? {
        guard !isLoop, days < Self.maxDays, let first = day.first?.coordinate, let last = day.last?.coordinate,
              let finishIndex = points.firstIndex(where: { $0.kind == .finish }) else { return nil }
        var points = points, order = routePoints.dropFirst().map(\.id)
        let night = points.filter { $0.kind == .night }.count + 1
        let end = points[finishIndex].coordinate
        points[finishIndex].kind = .night
        points[finishIndex].night = night
        points[finishIndex].id = "night-\(night)"
        order[order.count - 1] = "night-\(night)"
        var here = end
        if first.distance(to: end) > Trip.transferMinMeters {
            let id = "day-\(night + 1)"
            points.append(PlanPoint(id: id, label: "Start of day \(night + 1)", coordinate: first, kind: .via, leg: .transfer))
            order.append(id)
            here = first
        }
        points.append(PlanPoint(id: "finish", label: name ?? "Finish", coordinate: last, kind: .finish,
                                leg: .drawn, drawn: Self.drawnLeg(from: here, along: day)))
        points += waypoints.enumerated().map { index, waypoint in
            PlanPoint(id: "day-\(night + 1)-waypoint-\(index + 1)", label: waypoint.name, coordinate: waypoint.coordinate,
                      kind: .marker, placeKind: waypoint.category?.placeKind, note: waypoint.note)
        }
        var plan = self
        plan.points = points
        plan.routeOrder = order
        plan.days = night + 1
        plan.target = Double(night + 1)
        plan.mode = .trip
        return plan
    }
}

extension Trip {
    /// A new day end this close to an old one is the same place.
    public static let sameDayEndMeters = 50.0

    /// Makes the trip what `plan` says. `routed` is the line planned from it, and `pointIndices`
    /// gives the index in `routed` of each of the plan's route points in ride order, a loop's
    /// start again last. A transfer leg is no line: the leg after it starts a new piece. Each
    /// night ends a day; a night too close to the day end before it ends none. A day end that stays
    /// at the place of an old one keeps that day's name, place name and transfer label.
    public mutating func replacePlan(_ plan: PlannerPlan, line routed: [RoutePoint], pointIndices: [Int]) {
        let route = plan.routePoints + (plan.isLoop ? Array(plan.routePoints.prefix(1)) : [])
        guard route.count > 1, route.count == pointIndices.count, pointIndices.allSatisfy(routed.indices.contains) else { return }
        var line: [RoutePoint] = [], starts: [Int] = [], gap = false
        var nights: [(index: Int, label: String, resume: String?)] = []
        var startLabel = route[0].label
        for leg in 1..<route.count {
            let point = route[leg]
            if point.leg == .transfer {
                gap = true
                if nights.last?.index == line.count - 1 { nights[nights.count - 1].resume = point.label }
                if line.isEmpty { startLabel = point.label }
            } else {
                // A drawn leg can repeat its end points to keep their heights; a cut needs each point once.
                let piece = routed[pointIndices[leg - 1]...pointIndices[leg]].reduce(into: [RoutePoint]()) { piece, point in
                    if piece.last?.coordinate != point.coordinate { piece.append(point) }
                    else if piece[piece.count - 1].elevationMeters == nil { piece[piece.count - 1] = point }
                }
                if line.isEmpty { line = Array(piece) } else if gap {
                    starts.append(line.count)
                    line += piece
                } else {
                    line += piece.dropFirst()
                }
                gap = false
            }
            if point.kind == .night, !line.isEmpty { nights.append((line.count - 1, point.label, nil)) }
        }
        guard line.count > 1 else { return }
        let old = self
        let vertices = MeasuredLine(coordinates: line.map(\.coordinate), pieceStarts: starts).vertices
        let length = vertices[vertices.count - 1].distance
        var ends: [DayEnd] = []
        for night in nights {
            let distance = vertices[night.index].distance
            guard distance >= (ends.last?.distance ?? 0) + Self.minimumDayMeters, distance <= length - Self.minimumDayMeters
            else { continue }
            ends.append(DayEnd(coordinate: line[night.index].coordinate, name: Self.placeName(night.label), distance: distance,
                               resumeName: night.resume.flatMap(Self.placeName)))
        }
        ends.append(DayEnd(coordinate: line[line.count - 1].coordinate, name: Self.placeName(route[route.count - 1].label), distance: length))
        // A day end at the place of an old one keeps its day's name, its place name and its transfer label.
        let kept = ends.map { end in
            old.dayEnds.min { $0.coordinate.distance(to: end.coordinate) < $1.coordinate.distance(to: end.coordinate) }
                .flatMap { $0.coordinate.distance(to: end.coordinate) <= Self.sameDayEndMeters ? $0 : nil }
        }
        self.line = line
        pieceStarts = starts
        dayEnds = ends
        for (day, old) in kept.enumerated() {
            guard let old else { continue }
            dayEnds[day].title = old.title
            dayEnds[day].name = old.name ?? dayEnds[day].name
            if endsAtTransfer(day) { dayEnds[day].transfer = old.transfer }
        }
        startName = (line[0].coordinate == old.line.first?.coordinate ? old.startName : nil) ?? Self.placeName(startLabel)
        waypoints = plan.points.filter { $0.kind == .marker || $0.kind == .waypoint }
            .map { Stop(name: $0.label, coordinate: $0.coordinate) }
        self.plan = plan
    }

    private static func placeName(_ label: String) -> String? {
        PlannerPlan.isPlaceholder(label) ? nil : trimmed(label)
    }
}
