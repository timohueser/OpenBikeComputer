import Foundation
import OBCDomain
import Testing
import OBCTransport

/// A day note is keyed by the trip day or, without a trip, by the ride. It round-trips as plain
/// text, an empty note removes it, a ride's note goes with the ride, and the prompt row shows
/// until the note is written or the row is closed.
struct DayNoteStoreTests {
    enum StoreKind: CaseIterable { case inMemory, file }

    private func makeStore(_ kind: StoreKind) -> LibraryStore {
        switch kind {
        case .inMemory:
            return InMemoryLibraryStore()
        case .file:
            let dir = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("obc-note-tests-\(UUID().uuidString)", isDirectory: true)
            return FileLibraryStore(directory: dir)
        }
    }

    private func summary(_ id: String, trip: RideTrip? = nil) -> RideSummary {
        RideSummary(id: RideID(id), name: id, date: Date(timeIntervalSince1970: 1_000), distanceMeters: 10, trip: trip)
    }

    @Test func twoRidesOfOneTripDayShareTheKeyAndAStandaloneRideKeepsItsOwn() {
        let alps = RideTrip(key: 7, dayIndex: 1, dayCount: 3, name: "Alps traverse")
        let morning = summary("v4:a:1:s", trip: alps)
        let afternoon = summary("v4:a:2:s", trip: alps)
        let day3 = summary("v4:a:3:s", trip: RideTrip(key: 7, dayIndex: 2, dayCount: 3, name: "Alps traverse"))

        #expect(DayNoteKey(morning) == DayNoteKey(afternoon))
        #expect(DayNoteKey(morning) != DayNoteKey(day3))
        #expect(DayNoteKey(summary("v4:a:4:s")) == .ride(RideID("v4:a:4:s")))
    }

    @Test(arguments: StoreKind.allCases)
    func aNoteRoundTripsAndAnEmptyNoteRemovesIt(_ kind: StoreKind) {
        let store = makeStore(kind)
        let key = DayNoteKey.tripDay(key: 7, dayIndex: 1)
        let text = "Furka in the fog, then sun on the way down.\n\nWild camp by the lake, storm at 3."

        store.saveDayNote(text, for: key)
        #expect(store.dayNote(key) == text)
        #expect(store.dayNote(.tripDay(key: 7, dayIndex: 2)) == "")

        store.saveDayNote("", for: key)
        #expect(store.dayNote(key) == "")
    }

    @Test(arguments: StoreKind.allCases)
    func aDeletedRideTakesItsNoteAndLeavesTheTripDays(_ kind: StoreKind) throws {
        let store = makeStore(kind)
        let ride = summary("v4:a:1:s")
        try store.saveRide(Ride(
            summary: ride,
            points: [RidePoint(timestamp: ride.date, coordinate: Coordinate(latitude: 46, longitude: 8))]
        ))
        store.saveDayNote("Windy.", for: .ride(ride.id))
        store.saveDayNote("Day two.", for: .tripDay(key: 7, dayIndex: 1))

        store.deleteRide(ride.id)

        #expect(store.dayNote(.ride(ride.id)) == "")
        #expect(store.dayNote(.tripDay(key: 7, dayIndex: 1)) == "Day two.")
    }

    @Test func thePromptRowShowsUntilTheNoteIsWrittenOrTheRowIsClosed() {
        var journal = RideJournal()
        #expect(journal.offersNote(""))
        #expect(!journal.offersNote("Furka in the fog."))

        journal.close(.note)
        #expect(!journal.offersNote(""))
    }
}
