#if DEBUG
import Foundation
import OBCDomain

/// The phone's router without a network: a road that runs first east or west, then north or
/// south, as streets on a grid do. The first request fetches "map data" for a moment, later
/// ones answer at once. `failure` makes every request fail that way.
public actor MockLegRouter: LegRouter {
    private let failure: LegRouteFailure?
    private var hasData = false

    public init(failure: LegRouteFailure? = nil) {
        self.failure = failure
    }

    public func route(
        from: Coordinate, to: Coordinate, bikeType: BikeType, onDownload: @escaping @Sendable () -> Void
    ) async throws -> [RoutePoint] {
        if !hasData {
            onDownload()
            try await Task.sleep(for: .seconds(1.5))
            hasData = true
        }
        if let failure { throw failure }
        let corner = Coordinate(latitude: from.latitude, longitude: to.longitude)
        return [from, corner, to].map { RoutePoint(coordinate: $0) }
    }
}
#endif
