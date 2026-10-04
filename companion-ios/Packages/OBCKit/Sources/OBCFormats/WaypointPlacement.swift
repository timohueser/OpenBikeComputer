import Foundation
import OBCDomain

/// A waypoint as a route file carries it: named and positioned, with no ride order.
/// ``WaypointPlacement`` turns these into the ordered `Waypoint`s the app renders.
struct RawWaypoint {
    var name: String
    var note: String?
    let coordinate: Coordinate
    /// The icon the exporter wrote, kept verbatim; ``WaypointSymbol`` maps it to a
    /// category during placement.
    var symbol: String = ""
}

/// GPX carries waypoints file-level and unordered, so both formats place them with
/// ``Waypoint/placed(_:along:)``.
enum WaypointPlacement {
    static func place(_ raw: [RawWaypoint], along points: [RoutePoint]) -> [Waypoint] {
        Waypoint.placed(raw.map {
            Waypoint(index: 0, name: $0.name, note: $0.note, distanceAlongMeters: 0, coordinate: $0.coordinate,
                     category: WaypointSymbol.category(for: $0.symbol))
        }, along: points)
    }
}
