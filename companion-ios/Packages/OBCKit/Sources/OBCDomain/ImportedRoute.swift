import Foundation

/// A geographic point of a planned route — position + optional elevation, no time.
public struct RoutePoint: Hashable, Sendable {
    public let coordinate: Coordinate
    /// Elevation in metres, when the source carried one.
    public let elevationMeters: Double?
    /// Incoming segment surface class from OBCR; zero means unknown.
    public let surface: UInt8
    public let elevationIncomplete: Bool

    public init(coordinate: Coordinate, elevationMeters: Double? = nil, surface: UInt8 = 0, elevationIncomplete: Bool = false) {
        self.coordinate = coordinate
        self.elevationMeters = elevationMeters
        self.surface = surface & 7
        self.elevationIncomplete = elevationIncomplete
    }
}

/// A planned route as parsed from an interchange file: the canonical in-app model every import
/// format decodes into. Everything downstream consumes only this, so a new import format
/// touches exactly one `RouteFileDecoder` conformer.
public struct ImportedRoute: Equatable, Sendable {
    /// Route name carried by the file. `nil` when it had none; the UI then derives one.
    public var name: String?
    /// The authoring tool the file names (GPX `creator`, TCX author). `nil` when it does not say.
    public var creator: String?
    public var points: [RoutePoint]
    public var waypoints: [Waypoint]

    public init(
        name: String? = nil,
        creator: String? = nil,
        points: [RoutePoint],
        waypoints: [Waypoint] = []
    ) {
        self.name = name
        self.creator = creator
        self.points = points
        self.waypoints = waypoints
    }
}
