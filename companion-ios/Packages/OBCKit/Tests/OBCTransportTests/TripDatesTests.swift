import Testing
import Foundation
import OBCDomain

/// Dates follow the rides (`obc-ble-interface-spec.md` §7.7 "Day dates").
struct TripDatesTests {
    private func trip(days: Int, start: Int?) -> Trip {
        let files = (0..<days).map { k in
            [0.0, 0.01].map { RoutePoint(coordinate: Coordinate(latitude: 46.5, longitude: 8 + Double(k) * 0.01 + $0)) }
        }
        var trip = Trip.joining(files, id: TripID("t"), name: "T", bikeType: .road, now: Date())
        trip.startDay = start.map(CivilDay.init(daysSince1970:))
        return trip
    }

    private func days(_ values: [Int?]) -> [CivilDay?] { values.map { $0.map(CivilDay.init(daysSince1970:)) } }

    @Test
    func beforeAnyRideDaysCountFromTheStartDate() {
        #expect(trip(days: 3, start: 100).dayDates() == days([100, 101, 102]))
        #expect(trip(days: 2, start: nil).dayDates() == [nil, nil])
    }

    @Test
    func aLateRideMovesTheDaysAfterIt() {
        // Day 2 was ridden one day late; Day 3 follows it.
        #expect(trip(days: 3, start: 100).dayDates(finished: [0: CivilDay(daysSince1970: 100), 1: CivilDay(daysSince1970: 102)])
            == days([100, 102, 103]))
    }

    @Test
    func theLatestRideAtOrBeforeTheDayWins() {
        // Rides without a start date still date the days after them.
        #expect(trip(days: 4, start: nil).dayDates(finished: [1: CivilDay(daysSince1970: 200)]) == days([nil, 200, 201, 202]))
        // A ride of a later day re-anchors, even when it came early.
        #expect(trip(days: 4, start: 100).dayDates(finished: [0: CivilDay(daysSince1970: 103), 2: CivilDay(daysSince1970: 104)])
            == days([103, 104, 104, 105]))
    }

    @Test
    func aCivilDayIsTheRidersCalendarDay() {
        var tokyo = Calendar(identifier: .gregorian)
        tokyo.timeZone = TimeZone(identifier: "Asia/Tokyo")!
        // 2026-09-22 20:00 UTC is already 23 Sep in Tokyo.
        let instant = Date(timeIntervalSince1970: 1_790_107_200)
        let day = CivilDay(instant, calendar: tokyo)
        #expect(day == CivilDay(daysSince1970: 20_719))
        #expect(tokyo.dateComponents([.year, .month, .day, .hour], from: day.date(calendar: tokyo))
            == DateComponents(year: 2026, month: 9, day: 23, hour: 0))
    }
}
