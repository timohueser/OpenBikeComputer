import Testing
import Foundation
import OBCDomain

/// `RideMapLine` and `RideMapLines`: simplification, gaps, the zoom ladder and the tap hit test.
struct RideMapLineTests {
    /// Planar metres east and north of a fixed origin at 47° N.
    private func point(_ x: Double, _ y: Double) -> Coordinate {
        Coordinate(latitude: 47 + y / 111_320, longitude: 8 + x / (111_320 * cos(47 * Double.pi / 180)))
    }

    private func ridePoints(_ xy: [(Double, Double)], gapAt gap: Int? = nil) -> [RidePoint] {
        xy.enumerated().map { index, p in
            RidePoint(timestamp: Date(timeIntervalSince1970: Double(index)), coordinate: point(p.0, p.1),
                      segmentStart: index == gap)
        }
    }

    @Test
    func baseLevelDropsWobbleUnderTenMetresAndKeepsCorners() {
        // 1 km east with 3 m wobble, then 1 km north.
        let east = (0...100).map { (Double($0) * 10, $0.isMultiple(of: 2) ? 0.0 : 3.0) }
        let north = (1...100).map { (1000.0, Double($0) * 10) }
        let line = RideMapLine(id: RideID("a"), points: ridePoints(east + north))
        #expect(line.pieces.count == 1)
        #expect(line.pieces[0].count <= 4)
        #expect(line.pieces[0].first == point(0, 0))
        #expect(line.pieces[0].last == point(1000, 1000))
        #expect(line.distance(to: point(1000, 0)) < 5, "the corner survives")
    }

    @Test
    func aSegmentStartOpensANewPieceAndALonePointIsDropped() {
        let line = RideMapLine(
            id: RideID("a"),
            points: ridePoints([(0, 0), (500, 0), (2000, 0), (2500, 0), (4000, 0)], gapAt: 2)
        )
        #expect(line.pieces == [[point(0, 0), point(500, 0)], [point(2000, 0), point(4000, 0)]])
        #expect(line.distance(to: point(1250, 0)) > 700, "nothing is drawn across the gap")

        let lone = RideMapLine(id: RideID("b"), points: ridePoints([(0, 0), (500, 0), (900, 0)], gapAt: 2))
        #expect(lone.pieces == [[point(0, 0), point(500, 0)]])
    }

    @Test
    func aZoomPicksTheCoarsestLevelUnderOnePoint() {
        // A zigzag with 100 m teeth: kept at 40 m tolerance, flattened at 160 m.
        let zigzag = (0...40).map { (Double($0) * 200, $0.isMultiple(of: 2) ? 0.0 : 100.0) }
        let lines = RideMapLines([RideMapLine(id: RideID("a"), points: ridePoints(zigzag))])
        #expect(lines.lines(metersPerPoint: 5)[0].pieces[0].count == 41)
        #expect(lines.lines(metersPerPoint: 50)[0].pieces[0].count == 41)
        #expect(lines.lines(metersPerPoint: 200)[0].pieces[0].count == 2)
    }

    @Test
    func aTapFindsEveryRideWithinTheRadiusNearestFirst() {
        let lines = RideMapLines([
            RideMapLine(id: RideID("south"), points: ridePoints([(0, 0), (1000, 0)])),
            RideMapLine(id: RideID("north"), points: ridePoints([(0, 100), (1000, 100)])),
        ])
        let tap = point(500, 70)
        #expect(lines.rides(near: tap, withinMeters: 80, metersPerPoint: 2) == [RideID("north"), RideID("south")])
        #expect(lines.rides(near: tap, withinMeters: 20, metersPerPoint: 2).isEmpty)
        let southOnly = lines.restricted(to: [RideID("south")])
        #expect(southOnly.rides(near: tap, withinMeters: 80, metersPerPoint: 2) == [RideID("south")])
    }

    @Test
    func ridesOnTheSameRoadAllMatch() {
        let road = [(0.0, 0.0), (1000.0, 0.0)]
        let lines = RideMapLines([
            RideMapLine(id: RideID("monday"), points: ridePoints(road)),
            RideMapLine(id: RideID("friday"), points: ridePoints(road)),
        ])
        let found = lines.rides(near: point(500, 5), withinMeters: 20, metersPerPoint: 1)
        #expect(Set(found) == [RideID("monday"), RideID("friday")])
    }
}
