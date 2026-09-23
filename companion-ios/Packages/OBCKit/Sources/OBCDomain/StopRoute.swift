import Foundation

/// How a day that ends at a stop off the line reaches the stop. The day end itself stays on the
/// line point nearest the stop: that is the junction of an out and back, and the point a via
/// runs around.
public enum StopRoute: Equatable, Sendable {
    /// A spur from the junction to the stop. The next day rides it back to the same point, so the
    /// line does not change.
    case outAndBack(spur: [RoutePoint])
    /// The day leaves the line at `leave` and rides `toStop` to the stop; the next day rides
    /// `fromStop` and rejoins the line at `rejoin`. Nobody rides the line between the two.
    case via(toStop: [RoutePoint], fromStop: [RoutePoint], leave: Double, rejoin: Double)
}

/// Why the phone's router found no leg.
public enum LegRouteFailure: Error, Equatable, Sendable {
    /// The router needs a connection once for this area.
    case noConnection
    /// The map data for the area did not load.
    case mapData
    /// No road near an end, or no road between them within the device's search limit.
    case noRoad
}

/// The phone's router: the device's own router over map cells it fetches on demand. A seam, so
/// tests and the simulator route without a network.
public protocol LegRouter: Sendable {
    /// The road from `from` to `to` for `bikeType`, as the device would plan it. `onDownload` is
    /// called before the request fetches map data it does not have yet. Throws a
    /// ``LegRouteFailure``, or `CancellationError`.
    func route(
        from: Coordinate, to: Coordinate, bikeType: BikeType, onDownload: @escaping @Sendable () -> Void
    ) async throws -> [RoutePoint]
}
