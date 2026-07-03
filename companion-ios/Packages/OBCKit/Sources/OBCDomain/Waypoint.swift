import Foundation

/// A point of interest along a route. Waypoints travel with a `RouteBlob`
/// (route object side-table), not as a separate wire object.
public struct Waypoint: Identifiable, Equatable, Sendable {
    /// Ordinal position along the route (0-based, monotonic). Doubles as `id`.
    public let index: Int
    public var name: String
    /// Optional free-text note (nil when the source carried none).
    public var note: String?
    /// Cumulative distance from the route start, in metres.
    public let distanceAlongMeters: Double
    public let coordinate: Coordinate

    public var id: Int { index }

    public init(
        index: Int,
        name: String,
        note: String? = nil,
        distanceAlongMeters: Double,
        coordinate: Coordinate
    ) {
        self.index = index
        self.name = name
        self.note = note
        self.distanceAlongMeters = distanceAlongMeters
        self.coordinate = coordinate
    }
}
