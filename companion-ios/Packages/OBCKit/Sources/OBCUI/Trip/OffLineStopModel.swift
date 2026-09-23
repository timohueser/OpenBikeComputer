import Foundation
import Observation
import OBCDomain

/// The choice for a day that ends at a stop off the line: out and back, or via the stop. Both
/// are routed at once, so each option shows what it adds before the rider picks.
@MainActor @Observable
public final class OffLineStopModel: Identifiable {
    public enum Option: Equatable, Sendable {
        case routing
        /// Routed: the route and the metres it adds to the trip.
        case ready(StopRoute, extraMeters: Double)
        case failed(LegRouteFailure)
    }

    public enum Mode: Sendable {
        case outAndBack, via
    }

    public let day: Int
    public let stop: Stop
    /// Metres from the stop to the line.
    public let offset: Double
    /// The mode the day uses now, when it reaches the stop already.
    public let current: Mode?
    public private(set) var outAndBack: Option = .routing
    public private(set) var via: Option = .routing
    /// A request fetches map data for the area first.
    public private(set) var isDownloading = false

    @ObservationIgnored private let trip: Trip
    @ObservationIgnored private let router: any LegRouter
    @ObservationIgnored private let onPick: (StopRoute?) -> Void

    /// `trip`'s day `day` ends at a stop, on the line point nearest it.
    public init(trip: Trip, day: Int, router: any LegRouter, onPick: @escaping (StopRoute?) -> Void) {
        let end = trip.dayEnds[day]
        self.trip = trip
        self.day = day
        stop = end.stop ?? Stop(name: end.name ?? "", coordinate: end.coordinate, kind: .place)
        offset = end.stopOffset ?? 0
        current = switch end.stopRoute {
        case .outAndBack?: .outAndBack
        case .via?: .via
        case nil: nil
        }
        self.router = router
        self.onPick = onPick
    }

    /// Why nothing can be picked, once neither option routed. A missing connection explains more
    /// than a missing road, so it wins.
    public var failure: LegRouteFailure? {
        guard case .failed(let a) = outAndBack, case .failed(let b) = via else { return nil }
        return [a, b].contains(.noConnection) ? .noConnection : [a, b].contains(.mapData) ? .mapData : .noRoad
    }

    public func load() async {
        let signal: @Sendable () -> Void = { [weak self] in Task { @MainActor in self?.isDownloading = true } }
        let (trip, day, router) = (trip, day, router)
        async let out = Self.attempt { try await trip.routeOutAndBack(day, with: router, onDownload: signal) }
        async let through = Self.attempt { try await trip.routeVia(day, with: router, onDownload: signal) }
        let (a, b) = await (out, through)
        outAndBack = option(a)
        via = option(b)
        isDownloading = false
    }

    private nonisolated static func attempt(
        _ route: @Sendable () async throws -> StopRoute
    ) async -> Result<StopRoute, LegRouteFailure> {
        do {
            return .success(try await route())
        } catch {
            return .failure(error as? LegRouteFailure ?? .noRoad)
        }
    }

    private func option(_ result: Result<StopRoute, LegRouteFailure>) -> Option {
        switch result {
        case .success(let route): .ready(route, extraMeters: trip.extraMeters(route, at: day))
        case .failure(let failure): .failed(failure)
        }
    }

    public func pick(_ mode: Mode) {
        guard case .ready(let route, _) = mode == .outAndBack ? outAndBack : via else { return }
        onPick(route)
    }

    /// The day ends on the line point nearest the stop, named after it.
    public func endOnLine() {
        onPick(nil)
    }
}
