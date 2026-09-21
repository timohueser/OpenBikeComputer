import Foundation
import OBCDomain

/// One row of the Planned tab: a trip card or a loose route card. A route filed in a
/// trip never appears at the top level; it lives on its trip's page, so a route id is
/// in exactly one place. Trips and loose routes interleave by `addedAt`, newest first.
public enum PlannedItem: Identifiable, Equatable, Sendable {
    case trip(TripRecord)
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

    /// Partition the library into the interleaved top-level list: trips as trip cards,
    /// routes not filed in any trip as loose cards, newest first. Pure over its inputs,
    /// so the list model can be tested without a screen.
    ///
    /// `trips` are assumed already dangling-pruned, so a stage id that is not in
    /// `records` is filed-and-hidden, never shown twice.
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
