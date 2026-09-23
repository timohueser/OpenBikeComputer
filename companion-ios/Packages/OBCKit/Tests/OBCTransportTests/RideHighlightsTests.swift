import Foundation
import OBCDomain
import Testing

/// The highlight rules on a fixture ride with a known climb and descent.
struct RideHighlightsTests {
    /// One step east: its length, its duration and its change in elevation.
    private struct Step {
        var meters: Double
        var seconds: Double
        var climb: Double
    }

    private static let start = Date(timeIntervalSince1970: 1_790_000_000)

    /// A ride due east from 46.5° N at 500 m.
    private func ride(_ legs: [(step: Step, count: Int)], distance: Double = 0, trip: RideTrip? = nil) -> Ride {
        var x = 0.0, t = 0.0, e = 500.0
        var points = [point(x, t, e)]
        for (step, count) in legs {
            for _ in 0..<count {
                x += step.meters
                t += step.seconds
                e += step.climb
                points.append(point(x, t, e))
            }
        }
        let summary = RideSummary(id: RideID("fixture"), name: "Day 2", date: Self.start,
                                  distanceMeters: distance, trip: trip)
        return Ride(summary: summary, points: points)
    }

    private func point(_ x: Double, _ t: Double, _ e: Double) -> RidePoint {
        RidePoint(timestamp: Self.start.addingTimeInterval(t), coordinate: coordinate(x), elevationMeters: e)
    }

    private func coordinate(_ x: Double) -> Coordinate {
        Coordinate(latitude: 46.5, longitude: 8.4 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// 2 km flat; a 400 m climb over 5.1 km with a 15 m dip in it; a 3 km descent at 15 m/s with
    /// one 25 m/s sample; 1 km flat.
    private var pass: Ride {
        ride([
            (Step(meters: 100, seconds: 10, climb: 0), 20),
            (Step(meters: 100, seconds: 20, climb: 8), 25),
            (Step(meters: 100, seconds: 10, climb: -15), 1),
            (Step(meters: 100, seconds: 20, climb: 8.6), 25),
            (Step(meters: 150, seconds: 10, climb: -15), 10),
            (Step(meters: 250, seconds: 10, climb: -15), 1),
            (Step(meters: 150, seconds: 10, climb: -15), 9),
            (Step(meters: 100, seconds: 10, climb: 0), 10),
        ], distance: 82_000, trip: RideTrip(key: 7, dayIndex: 1, dayCount: 3, name: "Alps"))
    }

    @Test
    func aDipUnder20mDoesNotEndTheClimb() throws {
        let climb = try #require(RideHighlights.longestClimb(MeasuredLine(ridePoints: pass.points)))
        #expect(abs(climb.ascent - 400) < 0.01)
        #expect(abs(climb.length - 5_100) < 1)
    }

    @Test
    func aDipOf20mOrMoreEndsTheClimb() throws {
        let rollers = ride([
            (Step(meters: 100, seconds: 20, climb: 10), 10),
            (Step(meters: 100, seconds: 10, climb: -25), 1),
            (Step(meters: 100, seconds: 20, climb: 10), 5),
        ])
        let climb = try #require(RideHighlights.longestClimb(MeasuredLine(ridePoints: rollers.points)))
        #expect(abs(climb.ascent - 100) < 0.01)
        #expect(abs(climb.length - 1_000) < 1)
    }

    @Test
    func theDescentSpeedHolds20sSoOneFastSampleDoesNotSetIt() throws {
        let ride = pass
        let speed = try #require(RideHighlights.fastestDescent(ride.points, line: MeasuredLine(ridePoints: ride.points)))
        // The best 20 s window holds the 250 m sample and one 150 m sample, not the 25 m/s sample.
        #expect(abs(speed - 20) < 0.1)
    }

    @Test
    func theHighestPointTakesTheNameOfANearbyPlace() throws {
        let line = MeasuredLine(ridePoints: pass.points)
        let furka = Waypoint(index: 0, name: "Furka", distanceAlongMeters: 0, coordinate: coordinate(7_150))
        let far = Waypoint(index: 1, name: "Realp", distanceAlongMeters: 0, coordinate: coordinate(4_000))

        let named = try #require(RideHighlights.highestPoint(line, places: [far, furka]))
        #expect(named.place == "Furka")
        #expect(abs(named.elevation - 900) < 0.01)
        #expect(abs(named.distance - 7_100) < 1)
        #expect(RideHighlights.highestPoint(line, places: [far])?.place == nil)
    }

    @Test
    func atMostThreeTheMostNotableFirstWithTheBiggestDayOfItsOwnTrip() {
        let ride = pass
        let library = [
            ride.summary,
            RideSummary(id: RideID("day1"), name: "Day 1", date: Self.start, distanceMeters: 60_000,
                        trip: RideTrip(key: 7, dayIndex: 0, dayCount: 3, name: "Alps")),
            RideSummary(id: RideID("other"), name: "Other trip", date: Self.start, distanceMeters: 150_000,
                        trip: RideTrip(key: 8, dayIndex: 0, dayCount: 2, name: "Jura")),
        ]
        let highlights = RideHighlights.compute(ride, library: library)
        #expect(highlights.count == 3)
        guard highlights.count == 3 else { return }
        #expect(highlights[1] == .biggestDay(distance: 82_000))
        guard case .fastestDescent = highlights[0], case .longestClimb = highlights[2] else {
            Issue.record("expected descent, biggest day, climb; got \(highlights)")
            return
        }
    }

    @Test
    func aFlatRideHasNoHighlights() {
        let flat = ride([(Step(meters: 100, seconds: 10, climb: 0.5), 100)])
        #expect(RideHighlights.compute(flat).isEmpty)
    }
}
