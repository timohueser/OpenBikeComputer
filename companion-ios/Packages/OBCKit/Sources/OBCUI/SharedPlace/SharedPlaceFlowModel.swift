import Foundation
import Observation
import OBCDomain
import OBCPlanner
import OBCTransport

/// A place shared from a map app, on its way to the planner. The bike-type sheet opens at once, while
/// the place resolves and the position loads. The choice opens the planner when both are in.
@MainActor @Observable
public final class SharedPlaceFlowModel {
    /// The share that waits for the bike type: the place name, when the share has one, and the
    /// default type. Nil closes the sheet.
    public var pending: (name: String?, bikeType: BikeType)?
    /// Counts the shares, so a second share replaces the question on an open sheet. It stays when
    /// the sheet closes, so the sheet does not change while it animates out.
    public private(set) var shares = 0
    /// The bike type is chosen, and the place or the position is still on its way.
    public private(set) var isWaiting = false
    /// A planner to open. The composition root opens it and clears this.
    public var seed: PlannerPreviewSeed?
    /// One plain line about a share that cannot open the planner, for an alert.
    public var notice: String?

    @ObservationIgnored private var start: Task<(coordinate: Coordinate, name: String?)?, Never>?
    @ObservationIgnored private var finish: Task<(coordinate: Coordinate, name: String), any Error>?
    /// The choice made on the sheet; it runs once the sheet has gone.
    @ObservationIgnored private var chosen: BikeType?
    @ObservationIgnored private var opening: Task<Void, Never>?

    /// The rider's position, or nil when it is declined or unavailable.
    private let locate: @Sendable () async -> Coordinate?
    /// Names the start, such as with its locality.
    private let placeName: (@Sendable (Coordinate) async -> String?)?
    private let resolve: @Sendable (SharedPlace) async throws -> SharedPlace
    private let lastBikeType: LastBikeTypeStore

    public init(
        locate: @escaping @Sendable () async -> Coordinate?,
        placeName: (@Sendable (Coordinate) async -> String?)? = nil,
        resolve: @escaping @Sendable (SharedPlace) async throws -> SharedPlace = { try await SharedPlaceResolver().resolve($0) },
        lastBikeType: LastBikeTypeStore = LastBikeTypeStore()
    ) {
        self.locate = locate
        self.placeName = placeName
        self.resolve = resolve
        self.lastBikeType = lastBikeType
    }

    /// The shared text of an `openbikecomputer://place?text=…` link from the share extension; nil for
    /// any other link.
    public static func sharedText(in url: URL) -> String? {
        guard url.scheme == "openbikecomputer", url.host() == "place" else { return nil }
        let text = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "text" })?.value
        return text?.isEmpty == false ? text : nil
    }

    /// The text of a share: the sheet opens, and the place and the position start to load.
    public func receive(_ text: String, using source: any PlannerDataSource) {
        cancel()
        guard let place = SharedPlace(text: text) else {
            notice = Self.noPlace
            return
        }
        shares += 1
        pending = (place.name, lastBikeType.value)
        let locate = self.locate, placeName = self.placeName, resolve = self.resolve
        start = Task {
            guard let position = await locate() else { return nil }
            let name = await placeName?(position)
            return (position, name)
        }
        finish = Task { try await Self.destination(of: place, resolve: resolve, using: source) }
    }

    /// The bike type from the sheet. ``proceed()`` runs it once the sheet has gone.
    public func choose(_ bikeType: BikeType) {
        lastBikeType.value = bikeType
        chosen = bikeType
        pending = nil
    }

    /// Runs once the sheet has gone: the planner opens when the place and the position are in. A
    /// sheet closed without a choice drops the share.
    public func proceed() {
        guard let bikeType = chosen, let start, let finish else {
            cancel()
            return
        }
        chosen = nil
        isWaiting = true
        opening = Task { [weak self] in
            do {
                let place = try await finish.value
                let position = await start.value
                guard !Task.isCancelled else { return }
                self?.seed = PlannerPreviewSeed(start: position?.coordinate, startName: position?.name,
                                                finish: place.coordinate, finishName: place.name,
                                                activity: RouteActivity(bikeType))
            } catch {
                guard !Task.isCancelled else { return }
                self?.notice = error.localizedDescription
            }
            self?.isWaiting = false
        }
    }

    /// Drops the share, from the sheet or while it waits.
    public func cancel() {
        start?.cancel(); finish?.cancel(); opening?.cancel()
        start = nil; finish = nil; opening = nil; chosen = nil
        pending = nil
        isWaiting = false
    }

    static let noPlace = "No place found in this share."

    /// The place's position and name, inside the planner's map region.
    private static func destination(
        of place: SharedPlace, resolve: @Sendable (SharedPlace) async throws -> SharedPlace, using source: any PlannerDataSource
    ) async throws -> (coordinate: Coordinate, name: String) {
        let resolved: SharedPlace
        do { resolved = try await resolve(place) } catch is UnresolvedSharedLink {
            throw SharedPlaceProblem(errorDescription: noPlace)
        } catch {
            throw SharedPlaceProblem(errorDescription: "The map link could not be opened. Check your connection and share the place again.")
        }
        let found: (coordinate: Coordinate, name: String)
        switch resolved {
        case .coordinate(let coordinate, let name):
            found = (coordinate, name ?? place.name ?? "Shared place")
        case .query(let text):
            // A whole-world view, so the online search answers and not the first offline map. Without a
            // connection, the search without a view lets the offline maps answer.
            let release = try await source.release()
            let places: [PlannerPlace]
            do {
                places = try await source.search(PlannerSearchQuery(text: text, view: [-180, -90, 180, 90]), release: release)
            } catch {
                try Task.checkCancellation()
                places = try await source.search(PlannerSearchQuery(text: text), release: release)
            }
            guard let first = places.first else { throw SharedPlaceProblem(errorDescription: noPlace) }
            found = (first.coordinate, first.name)
        case .shortLink:
            throw SharedPlaceProblem(errorDescription: noPlace)
        }
        // A map covers a box, and a point has no area. `mapRelease(bounds:)` without `allowNetwork:`
        // binds statically to the protocol-extension default, which bypasses `LocalFirstPlanner`.
        let at = found.coordinate
        let box = [at.longitude - 1e-5, at.latitude - 1e-5, at.longitude + 1e-5, at.latitude + 1e-5]
        guard try await source.mapRelease(bounds: box, allowNetwork: true).contains(at) else {
            throw SharedPlaceProblem(errorDescription: "This place is outside the map region of the planner.")
        }
        return found
    }
}

private struct SharedPlaceProblem: LocalizedError {
    let errorDescription: String?
}
