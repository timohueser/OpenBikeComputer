import Foundation

/// A named place near a line: a campsite or a hotel from place search, another place the rider
/// searched for, or a waypoint from a trip's route files.
public struct Stop: Hashable, Sendable {
    /// The raw value is the stored name.
    public enum Kind: String, Hashable, Sendable {
        case campsite
        case hotel
        /// A search result of any other kind.
        case place
        /// A waypoint from a route file.
        case waypoint
    }

    public var name: String
    public var coordinate: Coordinate
    public var kind: Kind
    /// The place provider identifier, when the stop comes from search.
    public var mapItemID: String?

    public init(name: String, coordinate: Coordinate, kind: Kind, mapItemID: String? = nil) {
        self.name = name
        self.coordinate = coordinate
        self.kind = kind
        self.mapItemID = mapItemID
    }

    public init(waypoint: Waypoint) {
        self.init(name: waypoint.name, coordinate: waypoint.coordinate, kind: .waypoint)
    }
}

/// A stop measured against a trip line.
public struct PlacedStop: Hashable, Sendable {
    public var stop: Stop
    /// Metres along the line of the line point nearest the stop.
    public var distance: Double
    /// Metres from the stop to the line.
    public var offset: Double

    public init(stop: Stop, distance: Double, offset: Double) {
        self.stop = stop
        self.distance = distance
        self.offset = offset
    }

    public var isOnLine: Bool { offset <= Trip.onLineMeters }
}
