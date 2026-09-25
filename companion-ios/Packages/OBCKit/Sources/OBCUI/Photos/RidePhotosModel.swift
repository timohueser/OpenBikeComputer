import Foundation
import Observation
import OBCDomain
import OBCTransport

/// A ride's photos: the offer row, the pick grid, the strip, the pins and the ticks.
///
/// The app never adds a photo without the grid step, and it asks for library access only when
/// the rider chooses to add photos. Places come from the ride's points as they are now, so the model is
/// built again when the points change.
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
        public let placed: RidePhotoPlacement.Placed
        public var thumbnail: Data?
        public var id: String { placed.id }
    }

    public static let thumbnailPixels = 400

    public private(set) var offer: Offer?
    /// The added photos on the ride, in time order. A photo outside the ride's window is left out.
    public private(set) var placed: [RidePhotoPlacement.Placed] = []
    /// Keyed by asset id. A photo without one shows a placeholder until `fillThumbnails()`.
    public private(set) var thumbnails: [String: Data] = [:]
    /// The grid's photos; nil while they load.
    public private(set) var picks: [Pick]?
    public var selected: Set<String> = []
    /// The rider refused access; the view says where to allow it.
    public var accessDenied = false
    public private(set) var access: PhotoAccess
    public private(set) var canAddPhotos = false

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

    public var photos: [RidePhoto] { placed.map(\.photo) }

    /// Loads the ride's photos and the offer. A host may build throwaway models on every render,
    /// so this waits for the live one.
    public func start() async {
        guard !started, !points.isEmpty else { return }
        started = true
        journal = library.rideJournal(rideID)
        thumbnails = library.ridePhotoThumbnails(rideID)
        let points = points
        line = await Task.detached { MeasuredLine(ridePoints: points) }.value
        placed = await place(journal.photos)
        defer { canAddPhotos = true }
        guard !journal.closedRows.contains(.photos) else { return }
        switch access {
        case .notDetermined:
            offer = Offer(count: nil)
        case .denied:
            // Without access the app cannot tell which rides have photos.
            break
        case .limited, .full:
            let count = await placedCandidates().count
            // Limited access keeps the row at zero, so the rider can reach "Choose more photos…".
            offer = count > 0 ? Offer(count: count) : access == .limited ? Offer(count: nil) : nil
        }
    }

    // MARK: Offer and grid

    /// Refresh access on every open, including a return from the iPhone's Settings.
    public func openOffer() async -> Bool {
        guard canAddPhotos else { return false }
        access = photoLibrary.access()
        accessDenied = false
        if access == .notDetermined { access = await photoLibrary.requestAccess() }
        guard access.canRead else {
            accessDenied = true
            dismissOffer()
            return false
        }
        return true
    }

    /// Fills the grid, every new photo selected. Photos already in the grid keep their selection
    /// and their thumbnail.
    public func loadPicks() async {
        let known = Set(picks?.map(\.id) ?? [])
        let added = Set(journal.photos.map(\.assetID))
        let loaded = await placedCandidates().filter { !added.contains($0.id) }.map { placed in
            Pick(placed: placed, thumbnail: picks?.first { $0.id == placed.id }?.thumbnail)
        }
        selected.formUnion(loaded.map(\.id).filter { !known.contains($0) })
        selected.formIntersection(loaded.map(\.id))
        picks = loaded
    }

    /// Loads the grid's missing thumbnails. It stops when its task is cancelled.
    public func loadPickThumbnails() async {
        for pick in picks ?? [] where pick.thumbnail == nil {
            guard !Task.isCancelled else { return }
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

    /// Adds the selected photos. This uses the offer, so its row never returns. A thumbnail that
    /// has not loaded yet is filled later by `fillThumbnails()`.
    public func addSelected() {
        let chosen = (picks ?? []).filter { selected.contains($0.id) }
        journal.add(chosen.map(\.placed.photo))
        journal.close(.photos)
        var new: [String: Data] = [:]
        for pick in chosen { new[pick.id] = pick.thumbnail }
        let known = Set(placed.map(\.id))
        placed = (placed + chosen.map(\.placed).filter { !known.contains($0.id) })
            .sorted { $0.photo.takenAt < $1.photo.takenAt }
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

    /// Loads and keeps the thumbnails the strip is missing. It stops when its task is cancelled.
    public func fillThumbnails() async {
        for photo in photos where thumbnails[photo.assetID] == nil {
            guard !Task.isCancelled else { return }
            guard let data = try? await photoLibrary.image(photo.assetID, maxPixels: Self.thumbnailPixels)
            else { continue }
            save(thumbnails: [photo.assetID: data])
        }
    }

    public func remove(_ assetID: String) {
        journal.remove(assetID)
        placed.removeAll { $0.id == assetID }
        save(thumbnails: [:])
    }

    /// A screen-size image for the viewer; nil when the photo is deleted from the library.
    /// Throws `PhotoNotShared` when the access setting hides it.
    public func fullImage(_ assetID: String) async throws -> Data? {
        try await photoLibrary.image(assetID, maxPixels: 2_048)
    }

    /// Where each photo sits on the map, in `placed` order.
    public var pinCoordinates: [Coordinate] { placed.map(\.coordinate) }

    /// Where each photo sits on the profile, from 0 at the start to 1 at the end.
    public var tickFractions: [Double] {
        guard let length = line?.length, length > 0 else { return [] }
        return placed.map { $0.distanceMeters / length }
    }

    // MARK: Private

    private func placedCandidates() async -> [RidePhotoPlacement.Placed] {
        guard let window = RidePhotoPlacement.window(of: points), let line else { return [] }
        let candidates = await photoLibrary.candidates(takenIn: window)
        let points = points
        return await Task.detached { RidePhotoPlacement.place(candidates, on: points, line: line) }.value
    }

    private func place(_ photos: [RidePhoto]) async -> [RidePhotoPlacement.Placed] {
        guard let line, !photos.isEmpty else { return [] }
        let points = points
        return await Task.detached { RidePhotoPlacement.place(photos, on: points, line: line) }.value
    }

    private func save(thumbnails new: [String: Data]) {
        library.saveRideJournal(journal, thumbnails: new, for: rideID)
        let kept = Set(journal.photos.map(\.assetID))
        thumbnails = thumbnails.merging(new) { $1 }.filter { kept.contains($0.key) }
    }
}
