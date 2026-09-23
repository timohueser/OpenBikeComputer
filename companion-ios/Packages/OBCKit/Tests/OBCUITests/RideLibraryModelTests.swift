import Testing
import Foundation
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The Tracked list's library filter: the year it opens on, the chips it offers, and the map lines
/// it hands the map.
@MainActor
struct RideLibraryModelTests {
    private let utc: Calendar = {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }()

    private func ride(_ id: String, _ iso: String, _ type: BikeType) -> Ride {
        let date = ISO8601DateFormatter().date(from: iso)!
        return Ride(
            summary: RideSummary(id: RideID(id), name: id, date: date, distanceMeters: 1000, bikeType: type),
            points: [
                RidePoint(timestamp: date, coordinate: Coordinate(latitude: 47, longitude: 8)),
                RidePoint(timestamp: date, coordinate: Coordinate(latitude: 47.01, longitude: 8)),
            ]
        )
    }

    private func model(_ rides: [Ride], now: String = "2027-01-02T08:00:00Z") -> RideLibraryModel {
        let store = InMemoryLibraryStore()
        rides.forEach { store.saveRide($0) }
        let model = RideLibraryModel(
            library: store, calendar: utc, now: { ISO8601DateFormatter().date(from: now)! })
        model.rides = store.rideSummaries()
        return model
    }

    @Test
    func opensOnTheNewestRidesYearAndOffersTheCurrentOne() {
        let model = model([ride("a", "2026-09-01T08:00:00Z", .road), ride("b", "2025-06-01T08:00:00Z", .mtb)])
        #expect(model.year == 2026, "early January still opens on last season")
        #expect(model.years == [2027, 2026, 2025])
        #expect(model.filteredRides.map(\.name) == ["a"])
        #expect(model.bikeTypes == [.road])

        model.selectYear(nil)
        #expect(model.filteredRides.map(\.name) == ["a", "b"])
    }

    @Test
    func aYearWithoutTheChosenTypeFallsBackToAll() {
        let model = model([ride("a", "2026-09-01T08:00:00Z", .road), ride("b", "2025-06-01T08:00:00Z", .mtb)])
        model.selectYear(2025)
        model.selectBikeType(.mtb)
        #expect(model.totals.rideCount == 1)

        model.selectYear(2026)
        #expect(model.bikeType == nil)
        #expect(model.filteredRides.map(\.name) == ["a"])
    }

    @Test
    func mapLinesFollowTheFilter() async {
        let model = model([ride("a", "2026-09-01T08:00:00Z", .road), ride("b", "2026-06-01T08:00:00Z", .gravel)])
        #expect(model.filteredMapLines == nil)
        await model.loadMapLines()
        model.selectBikeType(.gravel)
        let ids = model.filteredMapLines?.lines(metersPerPoint: 1).map(\.id)
        #expect(ids == [RideID("b")])
    }
}
