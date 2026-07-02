import Foundation

/// A geographic point of a planned route — position + optional elevation, no time.
public struct RoutePoint: Hashable, Sendable {
    public let coordinate: Coordinate
    /// Elevation in metres, when the source carried one.
    public let elevationMeters: Double?

    public init(coordinate: Coordinate, elevationMeters: Double? = nil) {
        self.coordinate = coordinate
        self.elevationMeters = elevationMeters
    }
}

/// A planned route as parsed from an interchange file — the **canonical in-app
/// model** every import format (GPX, TCX, a future FIT course, …) decodes into.
/// Everything downstream — the import-landing detail (E1), stats, waypoints (W1),
/// and the device route encoder that produces the `RouteBlob` payload — consumes
/// only this, so adding an import format touches exactly one `RouteFileDecoder`
/// conformer (see `OBCFormats`).
public struct ImportedRoute: Equatable, Sendable {
    /// Route name carried by the file (`nil` when it had none — the UI derives one).
    public var name: String?
    /// The authoring tool the file names (GPX `creator`, TCX author) — feeds the
    /// E1 "Imported from Komoot" banner. `nil` when the file doesn't say.
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
