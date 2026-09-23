import Foundation
import Testing
import OBCDomain
import OBCTransport
@testable import OBCUI

/// The photo offer is a quiet row: it appears on a synced ride, goes when it is used or
/// dismissed, and never returns for that ride. The app reads the library only after access.
@MainActor @Suite struct RidePhotosModelTests {
    /// A library with photos at fixed times and a record of what the app asked.
    final class FakePhotos: PhotoLibrary, @unchecked Sendable {
        var current: PhotoAccess
        var grantOnRequest: PhotoAccess = .full
        var visible: [PhotoCandidate]
        var more: [PhotoCandidate] = []
        private(set) var queries = 0
        private(set) var requests = 0

        init(_ access: PhotoAccess, _ visible: [PhotoCandidate]) {
            current = access
            self.visible = visible
        }

        func access() -> PhotoAccess { current }

        func requestAccess() async -> PhotoAccess {
            requests += 1
            current = grantOnRequest
            return current
        }

        func candidates(takenIn range: ClosedRange<Date>) async -> [PhotoCandidate] {
            queries += 1
            return visible.filter { range.contains($0.takenAt) }
        }

        func image(_ assetID: String, maxPixels: Int) async throws -> Data? {
            Data(assetID.utf8)
        }

        func chooseMore() async {
            visible += more
        }
    }

    private static let start = Date(timeIntervalSince1970: 1_790_000_000)
    private static let rideID = RideID("ride")
    private static let points = (0...100).map {
        RidePoint(
            timestamp: start.addingTimeInterval(Double($0) * 10),
            coordinate: Coordinate(latitude: 46.5 + Double($0) * 10 / 111_320, longitude: 8.4)
        )
    }

    private static func candidate(_ id: String, at seconds: Double) -> PhotoCandidate {
        PhotoCandidate(assetID: id, takenAt: start.addingTimeInterval(seconds))
    }

    private static func library() throws -> InMemoryLibraryStore {
        let library = InMemoryLibraryStore()
        try library.saveRide(Ride(
            summary: RideSummary(id: rideID, name: "Day 2", date: start, distanceMeters: 1_000), points: points
        ))
        return library
    }

    private static func model(_ library: any LibraryStore, _ photos: FakePhotos) -> RidePhotosModel {
        RidePhotosModel(rideID: rideID, points: points, library: library, photoLibrary: photos)
    }

    @Test func withAccessTheRowCountsThePhotosTakenDuringTheRide() async throws {
        let photos = FakePhotos(.full, [Self.candidate("a", at: 100), Self.candidate("b", at: 500), Self.candidate("old", at: -3_600)])
        let model = Self.model(try Self.library(), photos)

        await model.start()

        #expect(model.offer?.title == "Add 2 photos from this ride")
    }

    @Test func beforeAccessTheRowHasNoCountAndTheLibraryIsNotRead() async throws {
        let photos = FakePhotos(.notDetermined, [Self.candidate("a", at: 100)])
        let model = Self.model(try Self.library(), photos)

        await model.start()

        #expect(model.offer?.title == "Add photos from this ride")
        #expect(photos.queries == 0)
        #expect(photos.requests == 0)
    }

    @Test func tappingTheRowAsksForAccessOnce() async throws {
        let photos = FakePhotos(.notDetermined, [Self.candidate("a", at: 100)])
        let model = Self.model(try Self.library(), photos)
        await model.start()

        #expect(await model.openOffer())
        #expect(await model.openOffer())

        #expect(photos.requests == 1)
    }

    @Test func refusedAccessOpensNoGrid() async throws {
        let photos = FakePhotos(.notDetermined, [])
        photos.grantOnRequest = .denied
        let model = Self.model(try Self.library(), photos)
        await model.start()

        #expect(await model.openOffer() == false)
        #expect(model.accessDenied)
    }

    /// Nothing is added before Add; then the chosen photos land in time order and the row goes
    /// for good.
    @Test func addingThePickedPhotosUsesTheRow() async throws {
        let library = try Self.library()
        let photos = FakePhotos(.full, [Self.candidate("b", at: 500), Self.candidate("a", at: 100), Self.candidate("c", at: 900)])
        let model = Self.model(library, photos)
        await model.start()
        await model.loadPicks()
        #expect(model.selected == ["a", "b", "c"])
        #expect(library.rideJournal(Self.rideID).photos.isEmpty)

        model.selected.remove("b")
        model.addSelected()

        #expect(model.offer == nil)
        #expect(model.photos.map(\.assetID) == ["a", "c"])
        #expect(model.thumbnails == ["a": Data("a".utf8), "c": Data("c".utf8)])
        let reopened = Self.model(library, photos)
        await reopened.start()
        #expect(reopened.offer == nil)
        #expect(reopened.photos.map(\.assetID) == ["a", "c"])
    }

    @Test func aDismissedRowNeverReturns() async throws {
        let library = try Self.library()
        let photos = FakePhotos(.full, [Self.candidate("a", at: 100)])
        let model = Self.model(library, photos)
        await model.start()

        model.dismissOffer()

        #expect(model.offer == nil)
        let reopened = Self.model(library, photos)
        await reopened.start()
        #expect(reopened.offer == nil)
        #expect(reopened.photos.isEmpty)
    }

    @Test func fullAccessWithNoPhotosShowsNoRow() async throws {
        let model = Self.model(try Self.library(), FakePhotos(.full, [Self.candidate("old", at: -3_600)]))

        await model.start()

        #expect(model.offer == nil)
    }

    /// Limited access counts only what the app can see, and keeps the row at zero so the rider
    /// can choose more.
    @Test func limitedAccessCountsTheVisiblePhotosAndChoosesMore() async throws {
        let empty = Self.model(try Self.library(), FakePhotos(.limited, []))
        await empty.start()
        #expect(empty.offer?.title == "Add photos from this ride")

        let photos = FakePhotos(.limited, [Self.candidate("a", at: 100)])
        photos.more = [Self.candidate("b", at: 200)]
        let model = Self.model(try Self.library(), photos)
        await model.start()
        #expect(model.offer?.title == "Add 1 photo from this ride")

        await model.loadPicks()
        await model.chooseMore()

        #expect(model.picks?.map(\.id) == ["a", "b"])
        #expect(model.selected == ["a", "b"])
    }

    @Test func removingAPhotoDropsItsPinAndTick() async throws {
        let library = try Self.library()
        let photos = FakePhotos(.full, [Self.candidate("a", at: 250), Self.candidate("b", at: 750)])
        let model = Self.model(library, photos)
        await model.start()
        await model.loadPicks()
        model.addSelected()
        #expect(model.tickFractions.map { ($0 * 100).rounded() } == [25, 75])

        model.remove("a")

        #expect(model.photos.map(\.assetID) == ["b"])
        #expect(model.pinCoordinates.count == 1)
        #expect(library.ridePhotoThumbnails(Self.rideID).keys.sorted() == ["b"])
    }
}
