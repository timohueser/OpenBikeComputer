import Foundation
import OBCCompanionCore
import OBCDomain

/// A map assembled in memory by the Rust core (`apps/obc-companion-core`), routed over by the
/// device's own router at the device's node limit.
final class AssembledMap {
    private let handle: OpaquePointer

    /// `catalog` is the catalog root; `job` names verified files (see `obc_core_assemble`).
    init(catalog: Data, job: Data) throws(RouteFailure) {
        let catalogText = String(decoding: catalog, as: UTF8.self)
        let jobText = String(decoding: job, as: UTF8.self)
        let handle = catalogText.withCString { c in jobText.withCString { j in obc_core_assemble(c, j) } }
        guard let handle else { throw .mapUnreadable(String(cString: obc_core_last_error())) }
        self.handle = handle
    }

    deinit { obc_core_map_free(handle) }

    func route(from: Coordinate, to: Coordinate, profile: UInt8) throws(RouteFailure) -> RoutedLeg {
        var out: OpaquePointer?
        let code = obc_core_route(handle, from.lonE6, from.latE6, to.lonE6, to.latE6, profile, &out)
        switch Int(code) {
        case OBC_CORE_ROUTED: break
        case OBC_CORE_NO_ROAD: throw .noRoad
        case OBC_CORE_EXHAUSTED: throw .exhausted
        case OBC_CORE_FAILED: throw .mapUnreadable(String(cString: obc_core_last_error()))
        default: throw .noPath
        }
        guard let route = out else { throw .noPath }
        defer { obc_core_route_free(route) }
        var count = 0
        let base = obc_core_route_points(route, &count)
        let points = UnsafeBufferPointer(start: base, count: count).map { p in
            RoutePoint(
                coordinate: Coordinate(latitude: Double(p.lat_udeg) / 1_000_000, longitude: Double(p.lon_udeg) / 1_000_000),
                elevationMeters: p.ele_m == Int16.min ? nil : Double(p.ele_m),
                surface: p.surface,
                elevationIncomplete: p.elevation_incomplete
            )
        }
        return RoutedLeg(
            points: points,
            distanceMeters: Int(obc_core_route_distance_m(route)),
            ascentMeters: Int(obc_core_route_ascent_m(route))
        )
    }
}

extension Coordinate {
    var latE6: Int32 { Int32((latitude * 1_000_000).rounded()) }
    var lonE6: Int32 { Int32((longitude * 1_000_000).rounded()) }
}
