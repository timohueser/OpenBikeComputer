import Foundation
import OBCDomain
import OBCTransport
import Testing
@testable import OBCUI

/// The trip review as the page reads it: one entry per ridden day with its note, the places the
/// trip does not name, and the re-balance offer that goes away for good once closed.
@MainActor
struct TripJournalModelTests {
    private let library = InMemoryLibraryStore()

    /// Planar metres east of a fixed origin at 46.5° N.
    private func coordinate(_ x: Double) -> Coordinate {
        Coordinate(latitude: 46.5, longitude: 8 + x / (111_320 * cos(46.5 * Double.pi / 180)))
    }

    private func file(_ from: Double, _ to: Double) -> [RoutePoint] {
        stride(from: from, through: to, by: 100).map { RoutePoint(coordinate: coordinate($0)) }
    }

    /// Day 1 0–10 km, a train to 11 km, then days 2–4 of 10 km each.
    private func trip() -> Trip {
        var trip = Trip.joining(
            [file(0, 10_000), file(11_000, 21_000), file(21_000, 31_000), file(31_000, 41_000)],
            id: TripID("t"), name: "Alps traverse", bikeType: .touring, now: Date(timeIntervalSince1970: 0))
        trip.namePlace(0, to: "Göschenen")
        trip.namePlace(1, to: "Reckingen")
        trip.setTransfer(0, to: .train)
        return trip
    }

    /// A ride of `day` from `from` to `to` km, saved to the library.
    private func ride(_ id: String, day: Int, of trip: Trip, from: Double, to: Double, hour: Double) throws -> RideSummary {
        let start = Date(timeIntervalSince1970: hour * 3_600)
        let points = stride(from: from * 1_000, through: to * 1_000, by: to >= from ? 20 : -20).enumerated().map {
            RidePoint(timestamp: start.addingTimeInterval(Double($0.offset) * 5), coordinate: coordinate($0.element))
        }
        let summary = RideSummary(
            id: RideID(id), name: id, date: start, distanceMeters: (to - from) * 1_000,
            trip: RideTrip(key: trip.key, dayIndex: day, dayCount: trip.dayCount, name: trip.name))
        try library.saveRide(Ride(summary: summary, points: points))
        return summary
    }

    @Test func entriesNameTheDaysAndTheOfferStaysClosed() async throws {
        let trip = trip()
        // Day 2 stops 7 km early, at 14 km.
        let rides = [
            try ride("d1", day: 0, of: trip, from: 0, to: 10, hour: 0),
            try ride("d2", day: 1, of: trip, from: 11, to: 14, hour: 24),
        ]
        library.saveDayNote("Train up the gorge.", for: .tripDay(key: trip.key, dayIndex: 0))
        let placeName: @Sendable (Coordinate) async -> String? = { $0.longitude < 8.16 ? "Andermatt" : "Ulrichen" }

        let journal = TripJournalModel(library: library, placeName: placeName)
        await journal.load(trip: trip, rides: rides)
        #expect(journal.entries.map(\.day) == [0, 1])
        #expect(journal.entries[0].note == "Train up the gorge.")
        #expect(journal.entries[1].header.contains("Andermatt → Reckingen"), "the day after the train starts at its own place")
        #expect(journal.transfers == [0: .init(from: "Göschenen", to: "Andermatt")])
        #expect(journal.offerTitle == "Days 3–4 are longer now. Even them out?")

        journal.closeOffer()
        #expect(journal.offer == nil)
        let again = TripJournalModel(library: library, placeName: placeName)
        await again.load(trip: trip, rides: rides)
        #expect(again.review?.rebalance != nil)
        #expect(again.offer == nil, "a closed offer never comes back")
    }

    @Test func afterAReverseTheTransferReadsFromTheOtherSide() async throws {
        var trip = trip()
        trip.reverse()
        // The reversed Day 3 runs from Reckingen to the train, which now leaves from Andermatt.
        let ride = try ride("d3", day: 2, of: trip, from: 21, to: 11, hour: 0)
        let journal = TripJournalModel(library: library) { $0.longitude < 8.16 ? "Andermatt" : "Ulrichen" }
        await journal.load(trip: trip, rides: [ride])

        let places = try #require(journal.transfers[2])
        #expect(TripJournalTransfer.text(kind: trip.dayEnds[2].transfer, from: places.from, to: places.to)
            == "Train · Andermatt → Göschenen")
    }

    @Test func aLoadThatANewerOneOvertookChangesNothing() async throws {
        let old = trip()
        var new = old
        new.setTransfer(0, to: .bus)
        let rides = [try ride("d1", day: 0, of: old, from: 0, to: 10, hour: 0)]
        let gate = Gate()
        // The first load waits in the geocoder until the second one has finished.
        let journal = TripJournalModel(library: library) { _ in await gate.pass() }

        let first = Task { await journal.load(trip: old, rides: rides) }
        while await !gate.isWaiting { await Task.yield() }
        await journal.load(trip: new, rides: rides)
        await gate.open()
        await first.value

        #expect(journal.trip?.dayEnds[0].transfer == .bus)
        #expect(journal.entries[0].header.contains("Late") == false, "the old load's answer is dropped")
    }
}

/// A geocoder whose first answer waits until the test opens it.
private actor Gate {
    private var waiter: CheckedContinuation<Void, Never>?
    private var first = true
    var isWaiting: Bool { waiter != nil }

    func pass() async -> String? {
        guard first else { return "Now" }
        first = false
        await withCheckedContinuation { waiter = $0 }
        return "Late"
    }

    func open() {
        waiter?.resume()
        waiter = nil
    }
}
