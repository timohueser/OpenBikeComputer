#if os(iOS)
import OBCDomain
import OBCPlanner
import SwiftUI

/// What the planner map draws while the Routes view is open.
extension PlannerRouteFinder {
    private static let mutedPlan = OBCTheme.amber.opacity(0.55), circleInk = OBCTheme.ink.opacity(0.7)

    /// The muted plan, the search circle, the listed routes in their network colours, and the selected route in magenta.
    func strokes(plan: [Coordinate]) -> [MapStroke] {
        var strokes = [MapStroke(coordinates: plan, color: Self.mutedPlan, width: 3, cased: false)]
        if let start {
            strokes.append(MapStroke(coordinates: Self.circle(start.coordinate, km: filters.radiusKm), color: Self.circleInk, width: 1, cased: false))
        }
        for match in matches.prefix(shown) {
            let color = PlannerPreviewNetworkStyle.lines[min(3, max(0, match.route.rank))]
            strokes.append(MapStroke(coordinates: line(match.route), color: color, width: 3, casingColor: OBCTheme.surface))
        }
        if let detail {
            strokes.append(MapStroke(coordinates: line(detail.route), color: OBCTheme.route, width: 4.5, casingColor: OBCTheme.surface))
        }
        return strokes
    }

    /// The map fits the circle for the list, and the route for a detail.
    var focus: [Coordinate] {
        if let detail, case let line = line(detail.route), !line.isEmpty { return line }
        return start.map { Self.circle($0.coordinate, km: filters.radiusKm) } ?? []
    }

    /// Numbered starts for the listed routes and dots for the other matches.
    var pins: [PlannerPreviewMapPin] {
        matches.enumerated().compactMap { index, match in
            match.route.start.map {
                PlannerPreviewMapPin(id: "route-\(match.route.id)", title: match.route.title, coordinate: $0,
                                     kind: .route(number: index < shown ? index + 1 : nil, rank: match.route.rank))
            }
        }
    }

    /// The line under the routes row of the search, such as "Gravel · loops within 10 km".
    func subtitle(activity: RouteActivity) -> String {
        let shape = switch filters.shape { case .loop: "loops"; case .oneWay: "routes"; case .any: "loops and routes" }
        return "\(activity.name) · \(shape) within \(Int(filters.radiusKm)) km"
    }

    private static func circle(_ center: Coordinate, km: Double) -> [Coordinate] {
        let dLat = km / 111.32, dLon = dLat / cos(center.latitude * .pi / 180)
        return (0...96).map { i in
            Coordinate(latitude: center.latitude + dLat * sin(Double(i) / 48 * .pi),
                       longitude: center.longitude + dLon * cos(Double(i) / 48 * .pi))
        }
    }
}
#endif
