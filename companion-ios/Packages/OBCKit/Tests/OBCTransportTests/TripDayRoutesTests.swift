import Testing
import Foundation
import OBCDomain
@testable import OBCTransport

/// The upload cut: one OBCR per day with its name and the trip's bike type, then the trip v3
/// object that names the day routes.
struct TripDayRoutesTests {
    private func file(_ fromKm: Double, _ toKm: Double) -> [RoutePoint] {
        stride(from: fromKm, through: toKm, by: 0.5).map { km in
            RoutePoint(
                coordinate: Coordinate(latitude: 46.5, longitude: 8 + km * 1000 / (111_320 * cos(46.5 * Double.pi / 180))),
                elevationMeters: 1000 + km * 20)
        }
    }

    private var trip: Trip {
        var trip = Trip.joining(
            [file(0, 10), file(10, 25), file(26, 30)], id: TripID("t"), name: "Alps", bikeType: .gravel,
            now: Date())
        trip.renameDay(1, to: "Ulrichen")
        return trip
    }

    @Test
    func eachDayIsAnOBCRWithItsNameTypeAndHeaderStats() throws {
        let days = trip.dayRoutes()
        #expect(days.map(\.name) == ["Day 1", "Day 2 Ulrichen", "Day 3"])
        for day in days {
            let decoded = try RouteObjectCodec.decode(day.payload)
            #expect(decoded.name == day.name)
            #expect(decoded.bikeType == .gravel)
            #expect(day.crc32 == CRC32.checksum(day.payload))
            #expect(day.estimatedDuration == BikeType.gravel.estimatedDuration(
                distanceMeters: day.distanceMeters, ascentMeters: day.elevationGainMeters))
        }
        #expect(abs(days[1].distanceMeters - 15_000) < 50)
        #expect(abs(days[1].elevationGainMeters - 300) < 5)
    }

    @Test
    func unchangedDaysKeepTheirBytes() {
        var changed = trip
        changed.renameDay(2, to: "Brig")
        let before = trip.dayRoutes().map(\.crc32)
        let after = changed.dayRoutes().map(\.crc32)
        #expect(before[0] == after[0] && before[1] == after[1])
        #expect(before[2] != after[2])
    }

    @Test
    func theNameCapCutsThePlaceNeverTheDayNumber() {
        let place = String(repeating: "Ä", count: 30)  // 60 UTF-8 bytes
        let name = Trip.routeName(day: 11, place: place)
        #expect(name.hasPrefix("Day 12 Ä"))
        #expect(name.utf8.count <= 48)
        #expect(name == "Day 12 " + String(repeating: "Ä", count: 20))
    }

    @Test
    func theTripObjectNamesEveryDayOnTheMainLine() throws {
        var trip = trip
        trip.startDay = CivilDay(daysSince1970: 20_725)
        let ids = [DeviceObjectID(7), DeviceObjectID(9), DeviceObjectID(4)]
        let decoded = try TripObjectCodec.decode(TripObjectCodec.encode(trip.tripObject(dayObjectIDs: ids)))
        #expect(decoded.key == trip.key)
        #expect(decoded.name == "Alps")
        #expect(decoded.startDate == 20_725)
        #expect(decoded.days == ids.map(TripObjectCodec.Day.whole))
        #expect(Trip(id: TripID("n"), name: "N", bikeType: .road, addedAt: Date()).tripObject(dayObjectIDs: []).startDate == 0)
    }
}
