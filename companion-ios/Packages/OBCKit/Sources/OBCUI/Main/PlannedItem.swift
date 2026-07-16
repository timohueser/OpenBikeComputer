import Foundation
import OBCDomain

/// One row of the Planned tab (TR6): a **trip** card or a **loose** route card.
/// Filed routes (members of some trip) never appear at the top level — they live
/// inside their trip's page — so a route id is in exactly one place. Trips and
/// loose routes interleave by `addedAt`, newest first, exactly as the flat route
/// list did before trips existed.
public enum PlannedItem: Identifiable, Equatable, Sendable {
    case trip(TripRecord)
    /// A top-level route with its library `addedAt` (the summary doesn't carry
    /// it) for the interleave sort.
    case route(RouteSummary, addedAt: Date)

    public var id: String {
        switch self {
        case .trip(let trip): "trip:\(trip.id.rawValue)"
        case .route(let summary, _): "route:\(summary.id.rawValue)"
        }
    }

    /// The library timestamp this item sorts by (newest first).
    public var sortDate: Date {
        switch self {
        case .trip(let trip): trip.addedAt
        case .route(_, let addedAt): addedAt
        }
    }

    /// The searchable/display name — the trip name or the route name.
    public var name: String {
        switch self {
        case .trip(let trip): trip.name
        case .route(let summary, _): summary.name
        }
    }

    /// Partition the library's planned routes and trips into the interleaved
    /// top-level list: trips as trip cards, routes **not filed in any trip** as
    /// loose cards, sorted by `addedAt` descending. Pure over its inputs so the
    /// list model can be unit-tested without a screen (TR6).
    ///
    /// `trips` are assumed already dangling-pruned (the `LibraryStore.trips()`
    /// contract), so a trip's `stageIDs` all resolve to a `records` entry; a
    /// stage id that somehow isn't in `records` is simply filed-and-hidden, never
    /// shown twice.
    public static func partition(
        records: [PlannedRouteRecord], trips: [TripRecord]
    ) -> [PlannedItem] {
        let filed = Set(trips.flatMap(\.stageIDs))
        var items: [PlannedItem] = trips.map(PlannedItem.trip)
        for record in records where !filed.contains(record.id) {
            items.append(.route(record.summary, addedAt: record.addedAt))
        }
        return items.sorted { $0.sortDate > $1.sortDate }
    }
}
