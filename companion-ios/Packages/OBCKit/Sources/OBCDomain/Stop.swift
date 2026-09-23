import Foundation

/// A place where a day of a trip can end: a campsite or a hotel from Apple Maps, another place
/// the rider searched for, or a waypoint from the trip's route files.
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
    /// The Apple Maps identifier (`MKMapItem.identifier`), when the stop came from Apple Maps.
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

extension Stop {
    /// The stops of several searches around one point, each stop once, in answer order. It fails
    /// only when every search failed: part of an answer is better than none.
    public static func merging(_ answers: [Result<[Stop], any Error>]) throws -> [Stop] {
        let found = answers.compactMap { try? $0.get() }
        if found.isEmpty, case .failure(let error)? = answers.first { throw error }
        var seen = Set<String>()
        return found.joined().filter { seen.insert($0.mapItemID ?? "\($0.name) \($0.coordinate)").inserted }
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

/// Apple Maps search, behind a seam so tests and the simulator run without a network.
public protocol StopSearch: Sendable {
    /// Campsites and hotels within `radius` metres of `center`.
    func stops(near center: Coordinate, radius: Double) async throws -> [Stop]
    /// Places of any kind that match `query`, inside the box from `southWest` to `northEast`.
    func places(matching query: String, southWest: Coordinate, northEast: Coordinate) async throws -> [Stop]
}
