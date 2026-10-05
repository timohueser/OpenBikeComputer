import Foundation
import Testing
@testable import OBCDomain
import OBCTransport

/// A trip made from its plan: the days, the transfers and the names a save in place derives.
struct TripPlanTests {
    /// Planar metres east of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double) -> Coordinate {
        Coordinate(latitude: 46.5, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight line east, a point every 100 m.
    private func points(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0)) }
    }

    private func point(_ id: String, _ label: String, _ x: Double, _ kind: PlanPoint.Kind, night: Int? = nil,
                       leg: PlanPoint.Leg? = nil) -> PlanPoint {
        PlanPoint(id: id, label: label, coordinate: coordinate(x), kind: kind, night: night, leg: leg)
    }

    @Test func savingAPlanMakesItsDaysAndKeepsTheTripsOwnFacts() throws {
        // Two days with a train after the first: the trip as it was.
        var trip = Trip.joining([points(0, 10_000), points(12_000, 30_000)], names: ["Furka", "Grimsel"],
                                id: TripID("t"), name: "Alps", bikeType: .touring, now: Date(timeIntervalSince1970: 0))
        trip.setTransfer(0, to: .train)
        trip.startDay = CivilDay(daysSince1970: 20_000)
        let before = trip

        // The plan adds a night at 20 km. The train is a transfer leg after the first night.
        let plan = PlannerPlan(points: [
            point("start", "Realp", 0, .start),
            point("a", "Spring", 5_000, .waypoint),
            point("night-1", "Göschenen", 10_000, .night, night: 1),
            point("day-2", "Andermatt", 12_000, .via, leg: .transfer),
            point("night-2", "Camp", 20_000, .night, night: 2),
            point("finish", "Brig", 30_000, .finish),
            point("m", "View", 15_000, .marker),
        ], mode: .trip, routeOrder: ["a", "night-1", "day-2", "night-2"])
        // Drawn legs repeat their end points with heights; the trip line has each point once.
        let night = RoutePoint(coordinate: coordinate(10_000), elevationMeters: 500)
        let routed = points(0, 10_000) + [night, RoutePoint(coordinate: coordinate(12_000))] + points(12_000, 30_000)
        trip.replacePlan(plan, line: routed, pointIndices: [0, 50, 101, 102, 183, 283])

        #expect(trip.dayCount == 3 && trip.pieceStarts == [101] && trip.line.count == 282 && trip.line[100] == night)
        #expect(trip.dayEnds.map { ($0.distance / 10).rounded() * 10 } == [10_000, 18_000, 28_000], "a transfer is no distance")
        #expect(trip.endsAtTransfer(0) && !trip.endsAtTransfer(1))
        #expect(trip.dayEnds.map(\.title) == ["Furka", nil, "Grimsel"], "a day name stays with its day end's place")
        #expect(trip.dayEnds.map(\.name) == ["Göschenen", "Camp", "Brig"])
        #expect(abs(TripStats(days: trip.dayRoutes()).distanceMeters - 28_000) < 100, "the day routes do not carry the transfer")
        #expect(trip.dayEnds[0].transfer == .train && trip.dayEnds[0].resumeName == "Andermatt")
        #expect(trip.dayStart(1)?.coordinate == coordinate(12_000))
        #expect(trip.startName == "Realp" && trip.waypoints.map(\.name) == ["Spring", "View"])
        #expect(trip.plan == plan)
        #expect(trip.id == before.id && trip.key == before.key && trip.startDay == before.startDay && trip.name == before.name)
        #expect(trip.dayLines().map { $0.points.first?.coordinate } == [coordinate(0), coordinate(12_000), coordinate(20_000)])
    }

    @Test func aNightWithNoRideBeforeItEndsNoDay() {
        var trip = Trip(id: TripID("t"), name: "T", bikeType: .road, addedAt: Date(timeIntervalSince1970: 0))
        let plan = PlannerPlan(points: [
            point("start", "A", 0, .start),
            point("night-1", "B", 10_000, .night, night: 1),
            point("night-2", "C", 12_000, .night, night: 2, leg: .transfer),
            point("finish", "D", 20_000, .finish),
        ], routeOrder: ["night-1", "night-2"])
        trip.replacePlan(plan, line: points(0, 10_000) + points(12_000, 20_000), pointIndices: [0, 100, 101, 181])
        #expect(trip.dayCount == 2 && trip.endsAtTransfer(0))
    }

    @Test func aDayAddedToAPlanEndsTheLastDayAndCrossesAGapByTransfer() throws {
        let plan = try #require(PlannerPlan.keptLine(points(0, 10_000)))
        let joined = try #require(plan.appendingDay(points(10_000, 20_000), name: "Brig"))
        #expect(joined.routePoints.map(\.kind) == [.start, .night, .finish] && joined.days == 2 && joined.mode == .trip)
        #expect(joined.routePoints.map(\.leg) == [nil, .drawn, .drawn] && joined.routePoints.last?.label == "Brig")
        let apart = try #require(joined.appendingDay(points(21_000, 30_000), name: nil))
        #expect(apart.routePoints.map(\.kind) == [.start, .night, .night, .via, .finish])
        #expect(apart.routePoints.map(\.leg) == [nil, .drawn, .drawn, .transfer, .drawn] && apart.days == 3)

        var full = apart
        while full.days < PlannerPlan.maxDays { full = try #require(full.appendingDay(points(30_000, 31_000), name: nil)) }
        #expect(full.appendingDay(points(30_000, 31_000), name: nil) == nil, "a plan holds at most 14 days")
        let long = Trip.joining((0..<15).map { points(Double($0) * 1_000, Double($0 + 1) * 1_000) }, id: TripID("t"), name: "T",
                                bikeType: .road, now: Date(timeIntervalSince1970: 0))
        #expect(long.dayCount == 15 && PlannerPlan.keptLine(long) == nil)
    }
}
