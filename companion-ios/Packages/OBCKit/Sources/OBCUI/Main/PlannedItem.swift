import Foundation
import OBCDomain

/// One row of the Planned tab: a trip card or a route card. A route added to a trip becomes part
/// of the trip's line, so it is no longer a route. Trips and routes interleave by `addedAt`,
/// newest first.
public enum PlannedItem: Identifiable, Equatable, Sendable {
    case trip(Trip)
    /// A top-level route with its library `addedAt`, which the summary does not carry,
    /// for the interleave sort.
    case route(RouteSummary, addedAt: Date)

    public var id: String {
        switch self {
        case .trip(let trip): "trip:\(trip.id.rawValue)"
        case .route(let summary, _): "route:\(summary.id.rawValue)"
        }
    }

    public var sortDate: Date {
        switch self {
        case .trip(let trip): trip.addedAt
        case .route(_, let addedAt): addedAt
        }
    }

    public var name: String {
        switch self {
        case .trip(let trip): trip.name
        case .route(let summary, _): summary.name
        }
    }

    /// The interleaved top-level list: trip cards and route cards, newest first.
    public static func partition(
        records: [PlannedRouteRecord], trips: [Trip]
    ) -> [PlannedItem] {
        let items = trips.map(PlannedItem.trip) + records.map { .route($0.summary, addedAt: $0.addedAt) }
        return items.sorted { $0.sortDate > $1.sortDate }
    }
}
