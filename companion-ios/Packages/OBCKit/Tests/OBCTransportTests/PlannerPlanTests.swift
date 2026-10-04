import Foundation
import Testing
import OBCDomain

/// The plan object of `specs/planner-plan.md`, and the kept line that a route or trip without a
/// plan opens as.
struct PlannerPlanTests {
    private func at(_ lon: Double, _ lat: Double = 46.5) -> Coordinate { Coordinate(latitude: lat, longitude: lon) }

    @Test func encodesTheFieldsOfTheSpec() throws {
        let plan = PlannerPlan(points: [
            PlanPoint(id: "start", label: "Brig", coordinate: at(8.0), progress: 0, kind: .start),
            PlanPoint(id: "night-1", label: "Camp", coordinate: at(8.1), progress: 0.4, kind: .night, night: 1,
                      placeKind: "camping", leg: .drawn, drawn: [RoutePoint(coordinate: at(8.05, 46.51), elevationMeters: 612), RoutePoint(coordinate: at(8.07, 46.52))]),
            PlanPoint(id: "v", label: "Shaping point", coordinate: at(8.2), progress: 0.6, kind: .via, turnaround: true),
            PlanPoint(id: "finish", label: "Sion", coordinate: at(8.3), progress: 1, kind: .finish, leg: .transfer),
            PlanPoint(id: "m", label: "View", coordinate: at(8.15, 46.6), progress: 0, kind: .marker, note: "Best at dusk"),
        ], mode: .route, bike: "gravel", preset: "Balanced", routeOrder: ["night-1", "v"])

        let json = try #require(JSONSerialization.jsonObject(with: JSONEncoder().encode(plan)) as? [String: Any])
        #expect(json["days"] as? Int == 2 && json["target"] as? Double == 2 && json["budget"] as? String == "days")
        #expect(json["limit"] as? Double == 50 && json["variant"] as? String == "valley" && json["mode"] as? String == "route")
        #expect(json["loop"] == nil && json["routeOrder"] as? [String] == ["night-1", "v"])
        let points = try #require(json["points"] as? [[String: Any]])
        #expect(points[0]["coordinate"] as? [Double] == [8.0, 46.5])
        #expect(points[0]["leg"] == nil && points[0]["turnaround"] == nil)
        #expect(points[1]["kind"] as? String == "night" && points[1]["night"] as? Int == 1 && points[1]["leg"] as? String == "drawn")
        #expect(points[1]["drawn"] as? [[Double]] == [[8.05, 46.51, 612], [8.07, 46.52]] && points[1]["placeKind"] as? String == "camping")
        #expect(points[2]["kind"] as? String == "via" && points[2]["turnaround"] as? Bool == true)
        #expect(points[3]["leg"] as? String == "transfer" && points[4]["note"] as? String == "Best at dusk" && points[0]["note"] == nil)

        let decoded = try JSONDecoder().decode(PlannerPlan.self, from: JSONEncoder().encode(plan))
        #expect(decoded == plan)
        #expect(decoded.routePoints.map(\.id) == ["start", "night-1", "v", "finish"] && decoded.markers.map(\.id) == ["m"])
    }

    @Test func aKeptLineIsOneDrawnLegThatFollowsTheLineAndKeepsItsHeights() throws {
        // A straight run with a corner: the run's inner vertices go, the corner stays. The ends keep
        // their heights in the line, since a plan point has none.
        let line = [(at(8.0), 300.0), (at(8.001), 301), (at(8.002), 302), (at(8.003), 310), (at(8.003, 46.501), 320), (at(8.003, 46.502), 330)]
            .map { RoutePoint(coordinate: $0.0, elevationMeters: $0.1, surface: 2) }
        let plan = try #require(PlannerPlan.keptLine(line))
        #expect(plan.points.map(\.kind) == [.start, .finish] && plan.days == 1 && plan.mode == .route)
        #expect(plan.points[0].coordinate == line[0].coordinate && plan.points[1].coordinate == line[5].coordinate)
        #expect(plan.points[1].leg == .drawn)
        #expect(plan.points[1].drawn == [0, 3, 5].map { RoutePoint(coordinate: line[$0].coordinate, elevationMeters: line[$0].elevationMeters) })
        let flat = try #require(PlannerPlan.keptLine(line.map { RoutePoint(coordinate: $0.coordinate) }))
        #expect(flat.points[1].drawn == [RoutePoint(coordinate: at(8.003))])
        #expect(PlannerPlan.keptLine([line[0]]) == nil)
    }

    @Test func waypointsPlaceOnTheSegmentsOfASparseLine() throws {
        // 6 km east in 10 m steps; the kept line has only its ends left.
        let east = 111_320 * cos(46.5 * .pi / 180)
        func metres(_ x: Double, _ y: Double = 0) -> Coordinate { Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / east) }
        let line = (0...600).map { RoutePoint(coordinate: metres(Double($0) * 10)) }
        let plan = try #require(PlannerPlan.keptLine(line))
        let kept = [plan.points[0].coordinate] + (plan.points[1].drawn ?? []).map(\.coordinate) + [plan.points[1].coordinate]
        #expect(kept.count <= 4)
        let placed = Waypoint.placed([
            Waypoint(index: 0, name: "Hut", distanceAlongMeters: 0, coordinate: metres(3_731, -150)),
            Waypoint(index: 0, name: "Spring", distanceAlongMeters: 0, coordinate: metres(1_492, 150)),
        ], along: kept.map { RoutePoint(coordinate: $0) })
        #expect(placed.map(\.name) == ["Spring", "Hut"])
        #expect(abs(placed[0].distanceAlongMeters - 1_492) < 3 && abs(placed[0].lateralOffsetMeters + 150) < 3)
        #expect(abs(placed[1].distanceAlongMeters - 3_731) < 3 && abs(placed[1].lateralOffsetMeters - 150) < 3)
    }

    @Test func aKeptTripHasANightAtEachDayEndAndATransferAcrossAGap() throws {
        let file = { (from: Double, to: Double) in stride(from: from, through: to, by: 0.005).map { RoutePoint(coordinate: self.at($0)) } }
        var trip = Trip.joining([file(8.0, 8.02), file(8.02, 8.04), file(8.10, 8.12)], id: TripID("t"), name: "Jura",
                                bikeType: .touring, now: Date(timeIntervalSince1970: 0))
        trip.namePlace(0, to: "Camp")
        let plan = try #require(PlannerPlan.keptLine(trip))
        let route = plan.routePoints
        #expect(route.map(\.kind) == [.start, .night, .night, .via, .finish] && plan.days == 3 && plan.mode == .trip)
        #expect(route.map(\.id) == ["start", "night-1", "night-2", "day-3", "finish"] && route[1].label == "Camp")
        #expect(route.map(\.leg) == [nil, .drawn, .drawn, .transfer, .drawn])
        #expect(route[3].coordinate == at(8.10) && route[2].coordinate == at(8.04))
        #expect(route.map(\.progress) == route.map(\.progress).sorted() && route.last?.progress == 1)
    }

    /// Imported files as days: a day that starts at the night before continues from it, a day that starts
    /// far away gets a transfer leg, and shaping points and markers stay.
    @Test func importedDaysJoinAtTheirNightsWithATransferAcrossAGap() throws {
        let line = { (from: Double, to: Double) in stride(from: from, through: to, by: 0.005).map { RoutePoint(coordinate: self.at($0)) } }
        let spring = Waypoint(index: 0, name: "Spring", distanceAlongMeters: 0, coordinate: at(8.03, 46.51), category: .water)
        let shaped = try #require(PlannerPlan.shaped([at(8.02), at(8.03), at(8.04)], turnarounds: [1], progress: [0, 0.5, 1],
                                                     waypoints: [spring]))
        let days = [try #require(PlannerPlan.keptLine(line(8.0, 8.02))), shaped, try #require(PlannerPlan.keptLine(line(8.10, 8.12)))]

        let plan = try #require(PlannerPlan.trip(days: days))
        let route = plan.routePoints
        #expect(route.map(\.kind) == [.start, .night, .via, .night, .via, .finish] && plan.days == 3 && plan.mode == .trip)
        #expect(route.map(\.id) == ["start", "night-1", route[2].id, "night-2", "day-3", "finish"] && route[2].turnaround == true)
        #expect(route.map(\.leg) == [nil, .drawn, nil, nil, .transfer, .drawn])
        #expect(route.map(\.progress) == route.map(\.progress).sorted() && route.last?.progress == 1)
        #expect(plan.markers.map(\.label) == ["Spring"] && plan.markers.map(\.placeKind) == ["water"])
    }

    /// One remaining day is still a trip; more days than a plan holds are no trip.
    @Test func aTripHasOneToFourteenDays() throws {
        let day = try #require(PlannerPlan.keptLine([RoutePoint(coordinate: at(8.0)), RoutePoint(coordinate: at(8.01))]))
        #expect(PlannerPlan.trip(days: [day])?.mode == .trip)
        #expect(PlannerPlan.trip(days: Array(repeating: day, count: 14)) != nil)
        #expect(PlannerPlan.trip(days: Array(repeating: day, count: 15)) == nil)
    }
}
