import Testing
import Foundation
import OBCDomain

/// `MeasuredLine`: distances along a line with gaps, lookups by distance, and the windowed
/// projection that keeps a dragged marker on its own leg of a switchback or an out-and-back.
struct MeasuredLineTests {
    /// Planar metres east and north of a fixed origin at 47° N.
    private func point(_ x: Double, _ y: Double) -> Coordinate {
        Coordinate(latitude: 47 + y / 111_320, longitude: 8 + x / (111_320 * cos(47 * Double.pi / 180)))
    }

    /// Two 1,000 m legs: east, then a 30 m hairpin north and back west.
    private var switchback: MeasuredLine {
        MeasuredLine(coordinates: [point(0, 0), point(1000, 0), point(1000, 30), point(0, 30)])
    }

    /// 2,000 m east and the same way back.
    private var outAndBack: MeasuredLine {
        MeasuredLine(coordinates: [point(0, 0), point(2000, 0), point(0, 0)])
    }

    @Test
    func distanceAccumulatesAndAGapCountsNothing() {
        let line = MeasuredLine(
            coordinates: [point(0, 0), point(1000, 0), point(3000, 0), point(4000, 0)],
            pieceStarts: [2]
        )
        #expect(abs(line.vertices[1].distance - 1000) < 2)
        #expect(abs(line.vertices[2].distance - 1000) < 2, "the jump into the second piece is free")
        #expect(abs(line.length - 2000) < 2)
    }

    @Test
    func lookupsInterpolateAndResolveAGapToThePieceEnd() {
        let line = MeasuredLine(
            coordinates: [point(0, 0), point(1000, 0), point(3000, 0), point(4000, 0)],
            elevations: [100, 200, nil, 300],
            pieceStarts: [2]
        )
        let mid = line.coordinate(at: 500)
        #expect(abs(mid.longitude - point(500, 0).longitude) < 1e-6)
        #expect(abs(line.elevation(at: 500) - 150) < 1)
        let boundary = line.vertices[1].distance
        #expect(line.index(at: boundary) == 1, "the shared distance belongs to the piece end")
        #expect(abs(line.coordinate(at: boundary).longitude - point(1000, 0).longitude) < 1e-6)
        #expect(abs(line.elevation(at: boundary) - 200) < 1e-9)
        #expect(abs(line.elevation(at: 1500) - 250) < 1, "a missing elevation repeats the last one")
    }

    @Test
    func climbComesFromTheCumulativeWalk() {
        let line = MeasuredLine(
            coordinates: (0..<6).map { point(Double($0) * 1000, 0) },
            elevations: [100, 101, 100.5, 105, 103, 110]
        )
        #expect(abs(line.climb(from: 0, to: line.length) - 10) < 0.01, "jitter inside the 3 m band is ignored")
        #expect(abs(line.climb(from: 3000, to: 5000) - 5) < 0.01)
        #expect(line.climb(from: 5000, to: 3000) == 0)
        #expect(line.descent(from: 0, to: line.length) == 0, "the 2 m dip stays inside the band")
    }

    @Test
    func projectionStaysOnItsLegOfASwitchback() {
        let line = switchback
        // A finger 10 m above the first leg, at x = 600, is also 20 m below the return leg.
        let finger = point(600, 10)
        let onFirstLeg = line.project(finger, near: 550, window: 200)
        #expect(abs(onFirstLeg - 600) < 2)
        // The same finger, with the marker on the return leg (x = 600 there is 1430 m along).
        let onReturnLeg = line.project(finger, near: 1400, window: 200)
        #expect(abs(onReturnLeg - 1430) < 2)
        // Without a window the nearer leg wins, whatever the marker's position.
        #expect(abs(line.project(finger, near: 1400, window: line.length) - 600) < 2)
    }

    @Test
    func projectionStaysOnItsLegOfAnOutAndBack() {
        let line = outAndBack
        let finger = point(500, 5)
        #expect(abs(line.project(finger, near: 400, window: 300) - 500) < 2)
        #expect(abs(line.project(finger, near: 3400, window: 300) - 3500) < 2, "the return leg passes the same place at 3,500 m")
    }

    @Test
    func projectionNeverLandsInsideAGapAndNeverLeavesTheWindow() {
        let line = MeasuredLine(
            coordinates: [point(0, 0), point(1000, 0), point(3000, 0), point(4000, 0)],
            pieceStarts: [2]
        )
        // Over the gap, nearer its far end: the near piece end wins because the far end is
        // outside the window, and the result is a real position.
        #expect(abs(line.project(point(2400, 0), near: 900, window: 800) - 1000) < 2)
        // With a window that reaches the far side, the far piece start (also at 1,000 m) wins.
        #expect(abs(line.project(point(2400, 0), near: 1000, window: 3000) - 1000) < 2)
        // A finger far ahead moves the marker at most one window per step.
        #expect(abs(line.project(point(3900, 0), near: 200, window: 300) - 500) < 2)
    }
}
