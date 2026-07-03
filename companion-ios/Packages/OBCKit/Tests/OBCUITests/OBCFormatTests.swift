import XCTest
import OBCDomain
@testable import OBCUI

/// The stat-line formatters must reproduce the design's strings exactly
/// (locale fixed to en_US).
final class OBCFormatTests: XCTestCase {
    private let en = Locale(identifier: "en_US")
    private var cal: Calendar {
        var cal = Calendar(identifier: .gregorian)
        cal.locale = en
        return cal
    }

    func testDistanceUsesOneDecimalUnder100km() {
        XCTAssertEqual(OBCFormat.distance(meters: 62_400, locale: en), "62.4 km")
        XCTAssertEqual(OBCFormat.distance(meters: 38_100, locale: en), "38.1 km")
    }

    func testDistanceDropsDecimalsAt100km() {
        XCTAssertEqual(OBCFormat.distance(meters: 118_000, locale: en), "118 km")
    }

    func testClimbGroupsThousands() {
        XCTAssertEqual(OBCFormat.climb(meters: 840, locale: en), "840 m ↑")
        XCTAssertEqual(OBCFormat.climb(meters: 1240, locale: en), "1,240 m ↑")
    }

    func testEstimatedDuration() {
        XCTAssertEqual(OBCFormat.estimatedDuration(3 * 3600 + 20 * 60), "3h 20m")
        XCTAssertEqual(OBCFormat.estimatedDuration(2 * 3600), "2h")
        XCTAssertEqual(OBCFormat.estimatedDuration(55 * 60), "55m")
        XCTAssertEqual(OBCFormat.estimatedDuration(48 * 3600), "2 days")
    }

    func testMovingTimeIsHColonMM() {
        XCTAssertEqual(OBCFormat.movingTime(2 * 3600 + 51 * 60), "2:51")
        XCTAssertEqual(OBCFormat.movingTime(65 * 60), "1:05")
    }

    func testSpeedFromMetersPerSecond() {
        XCTAssertEqual(OBCFormat.speed(mps: 20.4 / 3.6, locale: en), "20.4 kph")
    }

    func testRideDayLabels() {
        let now = date(2026, 7, 1, hour: 12)  // a Wednesday
        XCTAssertEqual(OBCFormat.rideDay(date(2026, 7, 1), relativeTo: now, calendar: cal, locale: en), "Today")
        XCTAssertEqual(OBCFormat.rideDay(date(2026, 6, 30), relativeTo: now, calendar: cal, locale: en), "Yesterday")
        // Friday inside the last week → short weekday.
        XCTAssertEqual(OBCFormat.rideDay(date(2026, 6, 26), relativeTo: now, calendar: cal, locale: en), "Fri")
        // Older than a week → short date.
        XCTAssertEqual(OBCFormat.rideDay(date(2026, 6, 12), relativeTo: now, calendar: cal, locale: en), "Jun 12")
    }

    func testPlannedSubtitleMatchesDesignRow() {
        let route = RouteSummary(
            id: RouteID("r1"),
            name: "Kettle Moraine Loop",
            distanceMeters: 62_400,
            elevationGainMeters: 840,
            estimatedDuration: 3 * 3600 + 20 * 60
        )
        XCTAssertEqual(OBCFormat.plannedSubtitle(route, locale: en), "62.4 km · 840 m ↑ · 3h 20m")
    }

    func testPlannedSubtitleOmitsMissingEstimate() {
        let route = RouteSummary(
            id: RouteID("r1"),
            name: "X",
            distanceMeters: 62_400,
            elevationGainMeters: 840
        )
        XCTAssertEqual(OBCFormat.plannedSubtitle(route, locale: en), "62.4 km · 840 m ↑")
    }

    func testTrackedSubtitleMatchesDesignRow() {
        let now = date(2026, 7, 1, hour: 12)
        let ride = RideSummary(
            id: RideID("d1"),
            name: "Kettle Moraine Loop",
            date: date(2026, 6, 30, hour: 8),
            distanceMeters: 58_200,
            movingTime: 2 * 3600 + 51 * 60,
            averageSpeedMps: 20.4 / 3.6
        )
        XCTAssertEqual(
            OBCFormat.trackedSubtitle(ride, relativeTo: now, calendar: cal, locale: en),
            "Yesterday · 58.2 km · 2:51 · 20.4 kph"
        )
    }

    // MARK: Stat-strip parts (value/unit split)

    func testStatValuesMatchTheJoinedLines() {
        XCTAssertEqual(OBCFormat.distanceValue(meters: 62_400, locale: en), "62.4")
        XCTAssertEqual(OBCFormat.distanceValue(meters: 118_000, locale: en), "118")
        XCTAssertEqual(OBCFormat.climbValue(meters: 1240, locale: en), "1,240")
        XCTAssertEqual(OBCFormat.speedValue(mps: 20.4 / 3.6, locale: en), "20.4")
    }

    func testRideDateLineMatchesE3Subtitle() {
        let now = date(2026, 7, 1, hour: 12)
        let line = OBCFormat.rideDateLine(
            date(2026, 6, 30, hour: 8, minute: 12), relativeTo: now, calendar: cal, locale: en
        )
        // DateFormatter separates "8:12" and "AM" with a narrow no-break space.
        XCTAssertEqual(line.replacingOccurrences(of: "\u{202F}", with: " "), "Yesterday, 8:12 AM")
    }

    // ------------------------------------------------------------- transfers

    func testMegabytesUseOneDecimal() {
        XCTAssertEqual(OBCFormat.megabytesValue(2_300_000, locale: en), "2.3")
        XCTAssertEqual(OBCFormat.megabytesValue(1_400_000, locale: en), "1.4")
        XCTAssertEqual(OBCFormat.megabytesValue(0, locale: en), "0.0")
        XCTAssertEqual(OBCFormat.megabytesValue(480_000, locale: en), "0.5")
    }

    func testTransferSizeLineMatchesDesignF() {
        XCTAssertEqual(
            OBCFormat.transferSizeLine(bytesDone: 1_400_000, totalBytes: 2_300_000, hasWaypoints: true, locale: en),
            "1.4 / 2.3 MB · route + waypoints"
        )
        XCTAssertEqual(
            OBCFormat.transferSizeLine(bytesDone: 0, totalBytes: 1_180_000, hasWaypoints: false, locale: en),
            "0.0 / 1.2 MB · route"
        )
    }

    func testTransferSizeLineUsesKilobytesForRealRoutes() {
        // A real OBCR route is tens of kB — MB would read a misleading "0.0".
        XCTAssertEqual(
            OBCFormat.transferSizeLine(bytesDone: 12_000, totalBytes: 24_000, hasWaypoints: true, locale: en),
            "12 / 24 kB · route + waypoints"
        )
        XCTAssertEqual(
            OBCFormat.transferSizeLine(bytesDone: 0, totalBytes: 3_200, hasWaypoints: false, locale: en),
            "0 / 3 kB · route"
        )
    }

    private func date(_ year: Int, _ month: Int, _ day: Int, hour: Int = 9, minute: Int = 0) -> Date {
        cal.date(from: DateComponents(year: year, month: month, day: day, hour: hour, minute: minute))!
    }
}
