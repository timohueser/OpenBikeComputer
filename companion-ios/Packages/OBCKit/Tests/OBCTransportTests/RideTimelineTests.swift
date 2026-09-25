import Foundation
import OBCDomain
import Testing

/// The timeline's series, plots and zones on rides due east with known spacing and timing.
struct RideTimelineTests {
    private static let start = Date(timeIntervalSince1970: 1_790_000_000)

    /// One fix: metres east of the start, seconds from it, and the sensor values.
    private struct Fix {
        var x: Double
        var t: Double
        var ele: Double? = 500
        var hr: Int?
        var power: Int?
        var cadence: Int?
        var segmentStart = false
    }

    private func ride(_ fixes: [Fix], limits: RideZoneLimits = .notSet) -> RideTimeline {
        let points = fixes.map { fix in
            RidePoint(
                timestamp: Self.start.addingTimeInterval(fix.t),
                coordinate: Coordinate(latitude: 46.5, longitude: 8.4 + fix.x / (111_320 * cos(46.5 * .pi / 180))),
                elevationMeters: fix.ele, heartRate: fix.hr, cadence: fix.cadence, power: fix.power,
                segmentStart: fix.segmentStart
            )
        }
        let summary = RideSummary(id: RideID("fixture"), name: "Ride", date: Self.start, distanceMeters: 0,
                                  zoneLimits: limits)
        return RideTimeline(ride: Ride(summary: summary, points: points))
    }

    @Test func channelsAreTheOnesWithSamples() {
        let timeline = ride([Fix(x: 0, t: 0, hr: 120), Fix(x: 100, t: 20), Fix(x: 200, t: 40)])
        #expect(timeline.channels == [.elevation, .speed, .heartRate])
        #expect(ride([Fix(x: 0, t: 0, ele: nil), Fix(x: 100, t: 20, ele: nil)]).channels == [.speed])
    }

    @Test func speedSpansTheWindowAndNeverAPauseGap() throws {
        // 5 m/s, then a gap into a second piece at 10 m/s.
        let timeline = ride([
            Fix(x: 0, t: 0), Fix(x: 5, t: 1), Fix(x: 10, t: 2),
            Fix(x: 500, t: 600, segmentStart: true), Fix(x: 510, t: 601),
        ])
        let stats = try #require(timeline.stats(.speed))
        #expect(abs(stats.min - 5) < 0.1)
        #expect(abs(stats.max - 10) < 0.1, "the jump into the second piece is not ridden")
    }

    @Test func theMeanWeighsSamplesByTime() throws {
        // Each fix holds half of the intervals beside it: the two 100 bpm fixes 30 s, the three
        // 200 bpm fixes 30 s. The plain mean would be 160.
        let timeline = ride([Fix(x: 0, t: 0, hr: 100), Fix(x: 10, t: 20, hr: 100), Fix(x: 20, t: 40, hr: 200),
                             Fix(x: 30, t: 50, hr: 200), Fix(x: 40, t: 60, hr: 200)])
        let stats = try #require(timeline.stats(.heartRate))
        #expect(stats.mean == 150)
        #expect(stats.min == 100 && stats.max == 200)
    }

    @Test func aPlotAveragesColumnsAndInterpolatesOnlyBetweenAdjacentFixes() {
        let timeline = ride([
            Fix(x: 0, t: 0, hr: 100), Fix(x: 10, t: 2, hr: 110),
            Fix(x: 400, t: 80, hr: 150),
            Fix(x: 800, t: 160, hr: nil), Fix(x: 1_000, t: 200, hr: 170),
        ])
        let plot = timeline.plot(.heartRate, columns: 10).values
        #expect(plot.count == 10)
        #expect(plot[0] == 105, "the first column averages its two fixes")
        // Column 2 is centred at 250 m, between the fixes at 10 m (110) and 400 m (150).
        #expect(abs((plot[2] ?? 0) - (110 + 40 * 240 / 390)) < 0.5, "a column between two fixes interpolates them")
        #expect(plot[6] == nil, "a dropout is a gap, not a ramp")
        #expect(plot[9] == 170)
    }

    @Test func timeInZonesCountsEachIntervalByItsSample() {
        let timeline = ride([
            Fix(x: 0, t: 0, hr: 100), Fix(x: 100, t: 60, hr: 100), Fix(x: 200, t: 90, hr: 175),
            Fix(x: 300, t: 900, hr: 175, segmentStart: true), Fix(x: 400, t: 960, hr: nil),
        ], limits: RideZoneLimits(maxHeartRate: 185, ftpWatts: nil))
        #expect(timeline.timeInZones(.heartRate) == [60, 0, 0, 0, 30])
        #expect(timeline.timeInZones(.power) == nil, "no FTP, no power zones")
        #expect(ride([Fix(x: 0, t: 0, hr: 100), Fix(x: 10, t: 5, hr: 100)]).timeInZones(.heartRate) == nil)
    }

    @Test func gradeBandsFollowTheClimbScreen() {
        #expect([-5, 2.9, 3, 5.9, 6, 9, 11.9, 12, 25].map { RideTimeline.gradeBand(percent: $0) } == [0, 0, 1, 1, 2, 3, 3, 4, 4])
    }
}
