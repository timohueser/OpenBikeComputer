import Testing
import Foundation
import OBCDomain
import OBCMock
import OBCTransport
@testable import OBCUI

/// Ride edits in the app: the Tracked list, the totals and the detail's points follow the edited
/// rides, and a merge suggestion stays gone once dismissed.
@MainActor
struct RideEditModelTests {
    private let t0 = Date(timeIntervalSince1970: 1_790_000_000)

    /// A ride north at 5 m/s, one point a second.
    private func ride(_ name: String, start: Date, seconds: Int, latitude: Double) -> Ride {
        Ride(
            summary: RideSummary(id: RideID(name), name: name, date: start, distanceMeters: 1),
            points: (0...seconds).map {
                RidePoint(timestamp: start.addingTimeInterval(Double($0)),
                          coordinate: Coordinate(latitude: latitude + Double($0) * 5 / 111_320, longitude: 8))
            }
        )
    }

    private func model(_ rides: [Ride]) -> (MainScreenModel, InMemoryLibraryStore) {
        let library = InMemoryLibraryStore()
        rides.forEach { library.saveRide($0) }
        let control = MockControl(scenario: .happyPath)
        control.latency = .zero
        let model = MainScreenModel(transport: MockTransport(control: control), library: library)
        model.start()
        model.rideLibrary.selectYear(nil)
        return (model, library)
    }

    @Test
    func totalsCountAnEditedRideOnce() throws {
        let morning = ride("Day 2 Ulrichen", start: t0, seconds: 600, latitude: 46.5)
        let lunch = ride("Day 2 Ulrichen (2)", start: t0.addingTimeInterval(3_600), seconds: 400,
                         latitude: morning.points.last!.coordinate.latitude + 0.002)
        let (model, _) = model([morning, lunch])
        #expect(model.rideLibrary.totals.rideCount == 2)

        #expect(model.mergeRideWithNext(morning.id))
        let merged = try #require(model.rides.first)
        #expect(model.rides.map(\.id) == [morning.id])
        #expect(model.rideLibrary.totals.rideCount == 1)
        #expect(model.rideLibrary.totals.distanceMeters == merged.distanceMeters)
        #expect(model.rideLibrary.totals.movingTime == 1_000, "the lunch break is not moving time")
        #expect(model.ride(morning.id)?.points.count == 601 + 401, "the detail reads the edited points")
        #expect(model.isEditedRide(morning.id))

        model.revertRide(morning.id)
        #expect(model.rides == [lunch.summary, morning.summary], "revert restores both rides exactly")
        #expect(!model.isEditedRide(morning.id))
    }

    @Test
    func aDismissedMergeSuggestionStaysGone() async {
        let morning = ride("Day 2 Ulrichen", start: t0, seconds: 600, latitude: 46.5)
        let lunch = ride("Day 2 Ulrichen (2)", start: t0.addingTimeInterval(3_600), seconds: 400,
                         latitude: morning.points.last!.coordinate.latitude + 0.002)
        let (model, library) = model([morning, lunch])
        #expect(await model.mergeSuggestion(for: morning.id)?.id == lunch.id)
        #expect(await model.mergeSuggestion(for: lunch.id) == nil, "no ride follows the second one")

        model.dismissMergeSuggestion(for: morning.id)
        #expect(await model.mergeSuggestion(for: morning.id) == nil)
        #expect(library.dismissedMerges() == [RidePair(first: morning.id, second: lunch.id)])
    }

    @Test
    func theAllRidesMapReadsTheEditedLine() async throws {
        let original = ride("a", start: t0, seconds: 600, latitude: 46.5)
        let (model, _) = model([original])
        await model.rideLibrary.loadMapLines()
        #expect(model.trimRide(original.id, to: t0...t0.addingTimeInterval(100)))
        await model.rideLibrary.loadMapLines()
        let line = try #require(model.rideLibrary.mapLines?.lines(metersPerPoint: 1).first)
        #expect(line.pieces.last?.last == original.points[100].coordinate)
    }

    @Test
    func aReDownloadedSyncedRideShowsThroughItsEdit() throws {
        let original = ride("a", start: t0, seconds: 600, latitude: 46.5)
        let (model, _) = model([original])
        #expect(model.trimRide(original.id, to: t0...t0.addingTimeInterval(100)))
        model.sync.onRideLanded(original)
        #expect(model.rides.count == 1)
        #expect(model.rides.first?.movingTime == 100, "the trim stays")
    }

    @Test
    func theEditScreenSavesOnlyWhatTheHandlesCut() throws {
        let edit = try #require(RideEditModel(
            ride: ride("a", start: t0, seconds: 600, latitude: 46.5), locale: Locale(identifier: "en_US")))
        #expect(!edit.canSave, "the trim handles start at the ends")

        // 5 m a second: 512 m is between the points at 102 s and 103 s.
        edit.editor.begin(0)
        edit.editor.move(0, to: 512)
        edit.editor.end()
        #expect(edit.trimRange == t0.addingTimeInterval(103)...t0.addingTimeInterval(600),
                "the cut part of a point interval does not stay")

        edit.editor.begin(1)
        edit.editor.move(1, to: 512)
        edit.editor.end()
        #expect(edit.trimRange == nil, "a kept part needs two points")
        #expect(!edit.canSave)
    }
}
