import Foundation

/// A named place on or near a trip line: a waypoint from a route file or a marker of the plan.
public struct Stop: Hashable, Sendable {
    public var name: String
    public var coordinate: Coordinate

    public init(name: String, coordinate: Coordinate) {
        self.name = name
        self.coordinate = coordinate
    }

    public init(waypoint: Waypoint) {
        self.init(name: waypoint.name, coordinate: waypoint.coordinate)
    }
}
