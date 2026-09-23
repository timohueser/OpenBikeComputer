import Foundation
import Observation
import OBCDomain
import OBCTransport

/// A ride's photos: the offer row, the pick grid, the strip, the pins and the ticks.
///
/// The app never adds a photo without the grid step, and it asks for library access only when
/// the rider taps the offer.
@MainActor @Observable
public final class RidePhotosModel {
    /// The quiet row's state; nil hides the row.
    public struct Offer: Equatable, Sendable {
        /// Nil before the app can read the library.
        public let count: Int?

        public var title: String {
            switch count {
            case nil: "Add photos from this ride"
            case 1: "Add 1 photo from this ride"
            case let count?: "Add \(count) photos from this ride"
            }
        }
    }

    /// One photo in the pick grid.
    public struct Pick: Identifiable, Equatable, Sendable {
        public let photo: RidePhoto
        public let locationOffTrack: Bool
        public var thumbnail: Data?
        public var id: String { photo.assetID }
    }

    public static let thumbnailPixels = 400

    public private(set) var offer: Offer?
    /// In time order.
    public private(set) var photos: [RidePhoto] = []
    /// Keyed by asset id. A photo without one shows a placeholder.
    public private(set) var thumbnails: [String: Data] = [:]
    /// The grid's photos; nil while they load.
    public private(set) var picks: [Pick]?
    public var selected: Set<String> = []
    /// The rider refused access; the view says where to allow it.
    public var accessDenied = false
    public private(set) var access: PhotoAccess

    public let rideID: RideID
    @ObservationIgnored private let points: [RidePoint]
    @ObservationIgnored private let library: any LibraryStore
    @ObservationIgnored private let photoLibrary: any PhotoLibrary
    @ObservationIgnored private var journal = RideJournal()
    @ObservationIgnored private var line: MeasuredLine?
    @ObservationIgnored private var started = false

    public init(rideID: RideID, points: [RidePoint], library: any LibraryStore, photoLibrary: any PhotoLibrary) {
        self.rideID = rideID
        self.points = points
        self.library = library
        self.photoLibrary = photoLibrary
        access = photoLibrary.access()
    }

    /// Loads the ride's photos and the offer. A host may build throwaway models on every render,
    /// so this waits for the live one.
    public func start() async {
        guard !started else { return }
        started = true
        journal = library.rideJournal(rideID)
        photos = journal.photos
        thumbnails = library.ridePhotoThumbnails(rideID)
        if !points.isEmpty { line = MeasuredLine(ridePoints: points) }
        guard !journal.closedRows.contains(.photos), line != nil else { return }
        guard access.canRead else {
            offer = Offer(count: nil)
            return
        }
        let count = await placedCandidates().count
        // Limited access keeps the row at zero, so the rider can reach "Choose more photos…".
        offer = count > 0 ? Offer(count: count) : access == .limited ? Offer(count: nil) : nil
    }

    // MARK: Offer and grid

    /// The rider tapped the offer. True when the grid should open.
    public func openOffer() async -> Bool {
        if access == .notDetermined { access = await photoLibrary.requestAccess() }
        guard access.canRead else {
            accessDenied = true
            return false
        }
        return true
    }

    /// Fills the grid, every photo selected. Photos that are already in the grid keep their
    /// selection.
    public func loadPicks() async {
        let known = Set(picks?.map(\.id) ?? [])
        let placed = await placedCandidates()
        var loaded = placed.map { Pick(photo: $0.photo, locationOffTrack: $0.locationOffTrack) }
        for index in loaded.indices {
            loaded[index].thumbnail = picks?.first { $0.id == loaded[index].id }?.thumbnail
        }
        selected.formUnion(loaded.map(\.id).filter { !known.contains($0) })
        selected.formIntersection(loaded.map(\.id))
        picks = loaded
        for pick in loaded where pick.thumbnail == nil {
            let data = try? await photoLibrary.image(pick.id, maxPixels: Self.thumbnailPixels)
            if let index = picks?.firstIndex(where: { $0.id == pick.id }) { picks?[index].thumbnail = data }
        }
    }

    /// The system picker for limited access, then the grid again with the new photos selected.
    public func chooseMore() async {
        await photoLibrary.chooseMore()
        await loadPicks()
    }

    public func closeGrid() {
        picks = nil
        selected = []
    }

    /// Adds the selected photos. This uses the offer, so its row never returns.
    public func addSelected() {
        let chosen = (picks ?? []).filter { selected.contains($0.id) }
        journal.add(chosen.map(\.photo))
        journal.close(.photos)
        var new: [String: Data] = [:]
        for pick in chosen { new[pick.id] = pick.thumbnail }
        save(thumbnails: new)
        offer = nil
        closeGrid()
    }

    public func dismissOffer() {
        journal.close(.photos)
        save(thumbnails: [:])
        offer = nil
    }

    // MARK: Strip and viewer

    public func remove(_ assetID: String) {
        journal.remove(assetID)
        save(thumbnails: [:])
    }

    /// A screen-size image for the viewer; nil when the photo is gone from the library.
    public func fullImage(_ assetID: String) async throws -> Data? {
        try await photoLibrary.image(assetID, maxPixels: 2_048)
    }

    /// Where each photo sits on the map, in `photos` order.
    public var pinCoordinates: [Coordinate] {
        guard let line else { return [] }
        return photos.map { line.coordinate(at: $0.distanceMeters) }
    }

    /// Where each photo sits on the profile, from 0 at the start to 1 at the end.
    public var tickFractions: [Double] {
        guard let line, line.length > 0 else { return [] }
        return photos.map { $0.distanceMeters / line.length }
    }

    // MARK: Private

    private func placedCandidates() async -> [RidePhotoPlacement.Placed] {
        guard let window = RidePhotoPlacement.window(for: points) else { return [] }
        return RidePhotoPlacement.place(await photoLibrary.candidates(takenIn: window), on: points)
    }

    private func save(thumbnails new: [String: Data]) {
        library.saveRideJournal(journal, thumbnails: new, for: rideID)
        photos = journal.photos
        thumbnails = thumbnails.merging(new) { $1 }.filter { key, _ in photos.contains { $0.assetID == key } }
    }
}
