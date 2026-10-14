import Foundation
import OBCDomain
import OBCPlanner

/// A large snap gap must stay visible. A manual outgoing leg already joins its selected point to the route.
func plannerRouteGapNote(_ path: PlannedPath, points: [Coordinate], legs: [PlanPoint.Leg]) -> String? {
    let gaps = zip(points, path.pointIndices).enumerated().compactMap { index, pair -> (Int, Double)? in
        if index == points.count - 1 && points.first == points.last { return nil }
        if index < legs.count && legs[index] != .routed { return nil }
        guard path.points.indices.contains(pair.1) else { return nil }
        let distance = pair.0.routeDistance(to: path.points[pair.1].coordinate)
        return distance > 100 ? (index, distance) : nil
    }
    guard let first = gaps.first, let maximum = gaps.map(\.1).max() else { return nil }
    let distance = maximum < 1000 ? "\(Int((maximum / 10).rounded()) * 10) m" : String(format: "%.1f km", maximum / 1000)
    let note = gaps.count > 1 ? "Route misses \(gaps.count) selected points by up to \(distance)"
        : first.0 == 0 ? "Route starts \(distance) from the selected point"
        : first.0 == points.count - 1 ? "Route ends \(distance) before the selected point"
        : "Route passes \(distance) from the selected point"
    return "\(note) — no accessible path"
}
