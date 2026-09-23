import Testing
import Foundation
import OBCDomain

/// The library filter and totals: which year a ride counts in, and which bike types appear.
struct RideLibraryTests {
    private let zurich: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Europe/Zurich")!
        return calendar
    }()

    private func ride(
        _ name: String, _ iso: String, km: Double, hours: Double = 1, climb: Double = 0,
        type: BikeType = .road
    ) -> RideSummary {
        RideSummary(
            id: RideID(name), name: name, date: ISO8601DateFormatter().date(from: iso)!,
            distanceMeters: km * 1000, movingTime: hours * 3600, climbMeters: climb, bikeType: type
        )
    }

    @Test
    func aRideCountsInTheYearItStartsInTheLocalTimeZone() {
        // 23:30 UTC on 31 December is 00:30 on 1 January in Zurich.
        let newYear = ride("night", "2025-12-31T23:30:00Z", km: 20)
        #expect(RideFilter(year: 2026).includes(newYear, calendar: zurich))
        #expect(!RideFilter(year: 2025).includes(newYear, calendar: zurich))
    }

    @Test
    func yearsOfferTheCurrentYearFirstEvenWithoutRides() {
        let rides = [ride("a", "2024-06-01T08:00:00Z", km: 10), ride("b", "2025-06-01T08:00:00Z", km: 10)]
        let now = ISO8601DateFormatter().date(from: "2026-03-01T08:00:00Z")!
        #expect(RideFilter.years(of: rides, now: now, calendar: zurich) == [2026, 2025, 2024])
    }

    @Test
    func totalsPerYearSplitByBikeTypeAndHideEmptyTypes() {
        let rides = [
            ride("a", "2026-05-01T08:00:00Z", km: 80, hours: 3, climb: 900, type: .road),
            ride("b", "2026-06-01T08:00:00Z", km: 60, hours: 4, climb: 1200, type: .gravel),
            ride("c", "2026-07-01T08:00:00Z", km: 40, hours: 2, climb: 300, type: .road),
            ride("d", "2025-07-01T08:00:00Z", km: 99, type: .mtb),
        ]
        let year = rides.filter { RideFilter(year: 2026).includes($0, calendar: zurich) }
        let all = RideTotals(year)
        #expect(all.rideCount == 3)
        #expect(all.distanceMeters == 180_000)
        #expect(all.movingTime == 9 * 3600)
        #expect(all.climbMeters == 2400)

        let byType = RideTotals.byBikeType(year)
        #expect(byType.map(\.bikeType) == [.road, .gravel], "MTB has rides only in 2025")
        #expect(byType[0].totals.distanceMeters == 120_000)
        #expect(byType[0].totals.rideCount == 2)
    }
}
