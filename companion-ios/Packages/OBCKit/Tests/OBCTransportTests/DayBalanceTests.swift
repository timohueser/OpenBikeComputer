import Testing
import Foundation
@testable import OBCDomain

/// The one balance-and-snap algorithm: equal riding time, a climb shortens its day, a stop
/// near an ideal end wins it, and day ends never cross.
struct DayBalanceTests {
    /// Planar metres east and north of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double, _ y: Double = 0) -> Coordinate {
        Coordinate(latitude: 46.5 + y / 111_320, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    /// A straight 30 km line east, a point every 100 m, with `elevation` at each point.
    private func line(elevation: ((Double) -> Double)? = nil) -> MeasuredLine {
        let xs = stride(from: 0.0, through: 30_000, by: 100).map { $0 }
        return MeasuredLine(coordinates: xs.map { coordinate($0) }, elevations: xs.map { elevation?($0) })
    }

    private func camp(_ name: String, _ x: Double, off: Double = 0) -> PlacedStop {
        PlacedStop(stop: Stop(name: name, coordinate: coordinate(x, off), kind: .campsite), distance: x, offset: off)
    }

    @Test
    func aFlatLineGivesEqualKilometres() {
        let ends = DayBalance.ends(on: line(), bikeType: .road, days: 3, candidates: [])
        #expect(ends.count == 2)
        #expect(abs(ends[0].distance - 10_000) < 1)
        #expect(abs(ends[1].distance - 20_000) < 1)
        #expect(ends.allSatisfy { $0.stop == nil })
    }

    @Test
    func aBigClimbShortensItsDay() {
        // 1,500 m of climb between km 5 and km 10, flat elsewhere.
        let climb = line { x in min(max(x - 5_000, 0), 5_000) * 0.3 }
        let ends = DayBalance.ends(on: climb, bikeType: .road, days: 3, candidates: [])
        let stats = climb.dayStats(ends: ends.map(\.distance), bikeType: .road)
        #expect(stats[0].distanceMeters < stats[2].distanceMeters, "the climbing day has fewer km")
        #expect(ends[0].distance < 10_000)
        let durations = stats.map(\.duration)
        #expect(durations.max()! - durations.min()! < 1, "the days are equal in riding time")
    }

    @Test
    func aStopNearTheIdealEndWinsIt() {
        // One day of a flat 30 km on a road bike is 1,636 s, so the window is about 1.5 km.
        let candidates = [
            camp("Far", 12_000),
            camp("Near", 10_800),
            camp("Nearest", 9_400),
            camp("OffLine", 10_200, off: 1_500),
        ]
        let ends = DayBalance.ends(on: line(), bikeType: .road, days: 3, candidates: candidates)
        #expect(ends[0].stop?.name == "Nearest", "of the candidates in the window, the one nearest along the line")
        #expect(ends[0].distance == 9_400)
        #expect(ends[1].stop == nil, "no candidate near km 20 keeps the ideal point")
        #expect(abs(ends[1].distance - 20_000) < 1)
    }

    @Test
    func endsNeverCrossAndNoDayFallsUnderTheFloor() {
        // Candidates at the far edges of both windows pull the ends toward each other.
        let candidates = [camp("A", 8_600), camp("B", 11_300), camp("C", 18_700), camp("D", 21_400)]
        let ends = DayBalance.ends(on: line(), bikeType: .road, days: 3, candidates: candidates)
        #expect(ends.map(\.stop?.name) == ["B", "C"])
        #expect(ends[0].distance < ends[1].distance)
        let stats = line().dayStats(ends: ends.map(\.distance), bikeType: .road)
        let average = stats.reduce(0) { $0 + $1.duration } / 3
        #expect(stats.allSatisfy { $0.duration >= DayBalance.minimumDayFraction * average })
    }

    @Test
    func aStretchBalancesOnlyItsPart() {
        let ends = DayBalance.ends(
            on: line(), bikeType: .road, days: 2, candidates: [camp("Before", 9_800)], stretch: 10_000...30_000)
        #expect(ends.count == 1)
        #expect(abs(ends[0].distance - 20_000) < 1)
        #expect(ends[0].stop == nil, "a candidate outside the stretch is no candidate")
    }

    @Test
    func costIsMonotoneAndItsInverseFindsTheDistance() {
        let climb = line { x in x * 0.05 }
        let half = climb.cost(to: 15_000, bikeType: .touring)
        #expect(abs(climb.distance(atCost: half, bikeType: .touring) - 15_000) < 1)
        #expect(climb.cost(to: 0, bikeType: .touring) == 0)
        #expect(abs(climb.cost(to: 40_000, bikeType: .touring) - climb.cost(to: 30_000, bikeType: .touring)) < 0.5, "held to the line")
    }
}
