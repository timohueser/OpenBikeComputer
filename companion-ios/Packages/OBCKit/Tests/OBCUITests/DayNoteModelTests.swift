import Foundation
import OBCDomain
import OBCTransport
import Testing
@testable import OBCUI

/// The prompt row shows on a synced ride until the writer opens or the row is dismissed, and
/// never again. The writer saves the trimmed text, an empty note removes it, and the header
/// takes the day's places from the trip or the geocoder.
@MainActor
struct DayNoteModelTests {
    private let library = InMemoryLibraryStore()
    private let day2 = RideTrip(key: 7, dayIndex: 1, dayCount: 3, name: "Alps traverse")

    private func ride(_ id: String, trip: RideTrip? = nil) throws -> RideSummary {
        let summary = RideSummary(
            id: RideID(id), name: "Ulrichen", date: Date(timeIntervalSince1970: 1_790_000_000),
            distanceMeters: 74_300, trip: trip
        )
        try library.saveRide(Ride(summary: summary, points: points))
        return summary
    }

    private var points: [RidePoint] {
        [
            RidePoint(timestamp: Date(timeIntervalSince1970: 1_790_000_000), coordinate: Coordinate(latitude: 46.63, longitude: 8.59)),
            RidePoint(timestamp: Date(timeIntervalSince1970: 1_790_020_000), coordinate: Coordinate(latitude: 46.50, longitude: 8.30)),
        ]
    }

    private func model(_ ride: RideSummary, placeName: (@Sendable (Coordinate) async -> String?)? = nil) -> DayNoteModel {
        DayNoteModel(ride: ride, points: points, library: library, placeName: placeName)
    }

    @Test func theRowShowsUntilTheWriterOpensAndThenNeverAgain() async throws {
        let ride = try ride("a", trip: day2)
        let first = model(ride)
        await first.start()
        #expect(first.offer)
        #expect(first.prompt == "How was Day 2?")
        #expect(first.title == "Day 2")

        first.openWriter()
        #expect(!first.offer)

        let again = model(ride)
        await again.start()
        #expect(!again.offer)
        #expect(again.dismissed)
    }

    @Test func aDismissedRowStaysGone() async throws {
        let ride = try ride("a")
        let first = model(ride)
        await first.start()
        first.dismissOffer()

        let again = model(ride)
        await again.start()
        #expect(!again.offer)
        #expect(again.prompt == "How was the ride?")
    }

    @Test func theWriterSavesTheTrimmedTextAndAnEmptyNoteRemovesIt() async throws {
        let ride = try ride("a", trip: day2)
        let writer = model(ride)
        await writer.start()
        writer.openWriter()
        writer.draft = "  Furka in the fog.\n"
        writer.save()
        #expect(writer.note == "Furka in the fog.")
        #expect(library.dayNote(.tripDay(key: 7, dayIndex: 1)) == "Furka in the fog.")

        writer.draft = "   "
        writer.save()
        #expect(library.dayNote(.tripDay(key: 7, dayIndex: 1)) == "")
    }

    @Test func aKeystrokeSavesAfterAPause() async throws {
        let ride = try ride("a")
        let writer = model(ride)
        await writer.start()
        writer.draft = "Windy."
        writer.draftChanged()
        #expect(library.dayNote(.ride(ride.id)) == "")
        try await Task.sleep(for: DayNoteModel.saveDelay * 2)
        #expect(library.dayNote(.ride(ride.id)) == "Windy.")
    }

    @Test func anotherRideOfTheDayShowsTheNoteAndNoRow() async throws {
        let morning = try ride("a", trip: day2)
        let afternoon = try ride("b", trip: day2)
        let writer = model(morning)
        await writer.start()
        writer.draft = "Furka in the fog."
        writer.save()

        let other = model(afternoon)
        await other.start()
        #expect(other.note == "Furka in the fog.")
        #expect(!other.offer)
    }

    @Test func theHeaderTakesTheDaysPlacesFromTheTrip() async throws {
        let trip = Trip(
            id: TripID("t"), name: "Alps traverse", bikeType: .road,
            dayEnds: [
                DayEnd(coordinate: Coordinate(latitude: 46.6, longitude: 8.6), name: "Andermatt", distance: 0),
                DayEnd(coordinate: Coordinate(latitude: 46.5, longitude: 8.3), name: "Ulrichen", distance: 0),
            ],
            startName: "Realp", addedAt: Date()
        )
        library.saveTrip(trip)
        let ride = try ride("a", trip: RideTrip(key: trip.key, dayIndex: 1, dayCount: 2, name: trip.name))

        let day = model(ride) { _ in "Geocoded" }
        await day.start()
        #expect(day.header.hasSuffix(" · Andermatt → Ulrichen · 74 km"))
    }

    @Test func aDayAfterATransferStartsAtItsOwnPlace() async throws {
        // Day 1 ends at Göschenen; the train takes the rider on to Andermatt.
        func file(_ lat: Double, _ lon: Double) -> [RoutePoint] {
            [0, 0.01].map { RoutePoint(coordinate: Coordinate(latitude: lat, longitude: lon + $0)) }
        }
        var trip = Trip.joining(
            [file(46.67, 8.58), file(46.63, 8.59)], id: TripID("t"), name: "Alps traverse", bikeType: .road, now: Date())
        trip.namePlace(0, to: "Göschenen")
        trip.namePlace(1, to: "Ulrichen")
        #expect(trip.endsAtTransfer(0))
        library.saveTrip(trip)
        let ride = try ride("a", trip: RideTrip(key: trip.key, dayIndex: 1, dayCount: 2, name: trip.name))

        let day = model(ride) { $0.latitude > 46.6 ? "Andermatt" : "Brig" }
        await day.start()
        #expect(day.header.hasSuffix(" · Andermatt → Ulrichen · 74 km"))
    }

    @Test func aRideWithoutATripAsksTheGeocoder() async throws {
        let ride = try ride("a")
        let lone = model(ride) { $0.latitude > 46.6 ? "Andermatt" : "Ulrichen" }
        await lone.start()
        #expect(lone.header.hasSuffix(" · Andermatt → Ulrichen · 74 km"))

        let offline = model(ride)
        await offline.start()
        #expect(offline.header.hasSuffix(" · 74 km"))
        #expect(!offline.header.contains("→"))
    }
}
