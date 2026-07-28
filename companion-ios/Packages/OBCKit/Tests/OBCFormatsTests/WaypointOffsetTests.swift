import Testing
import OBCDomain
@testable import OBCFormats

/// The signed lateral offset the placement fixes at import (`OBCR_Spec.md` §4,
/// #947): magnitude to the track point that won the placement, sign = which side
/// of the direction of travel the waypoint fell on, **positive = right**. The
/// firmware converter derives it the same way from the same nearest-point rule,
/// so these cases mirror `obc-route`'s offset tests.
struct WaypointOffsetTests {
    /// A dead-straight **eastward** track at 47°N: three points, ~152 m apart.
    private let eastward = [
        RoutePoint(coordinate: Coordinate(latitude: 47.000, longitude: 11.000), elevationMeters: nil),
        RoutePoint(coordinate: Coordinate(latitude: 47.000, longitude: 11.002), elevationMeters: nil),
        RoutePoint(coordinate: Coordinate(latitude: 47.000, longitude: 11.004), elevationMeters: nil),
    ]

    private func offset(of latitude: Double, _ longitude: Double) -> Double {
        let raw = [RawWaypoint(
            name: "W", note: nil, coordinate: Coordinate(latitude: latitude, longitude: longitude)
        )]
        return WaypointPlacement.place(raw, along: eastward)[0].lateralOffsetMeters
    }

    /// Riding east, north is **left** (negative) and south is **right** (positive);
    /// 0.001° of latitude is ~111 m.
    @Test("the sign is the side of travel")
    func signIsTheSideOfTravel() {
        let north = offset(of: 47.001, 11.002)
        let south = offset(of: 46.999, 11.002)
        #expect(north < 0, "north of an eastbound track is on the left")
        #expect(south > 0, "south of an eastbound track is on the right")
        #expect(abs(north + south) < 0.01, "mirrored offsets, opposite signs")
        #expect(abs(south - 111.19) < 0.5, "0.001° of latitude ≈ 111 m")
    }

    @Test("a waypoint on the line is on-route")
    func onTheLineIsZero() {
        #expect(offset(of: 47.000, 11.002) == 0)
    }

    /// The winning point can be the track's **first**, which has no incoming
    /// segment — the side then comes from the outgoing one (the firmware resolves
    /// that case the same way, one point later in its streaming pass).
    @Test("the first track point still takes a side")
    func firstPointTakesASide() {
        #expect(offset(of: 47.001, 11.000) < 0, "beside the start, north is still left")
        #expect(offset(of: 46.999, 11.000) > 0, "beside the start, south is still right")
    }

    /// Placement is also where the source symbol becomes a category — and where an
    /// unmapped one degrades to generic **without dropping the waypoint**.
    @Test("placement categorizes from the symbol")
    func placementCategorizesFromTheSymbol() {
        let raw = [
            RawWaypoint(name: "Fountain", note: nil, coordinate: eastward[0].coordinate, symbol: "Drinking Water"),
            RawWaypoint(name: "Turn", note: nil, coordinate: eastward[1].coordinate, symbol: "Flag, Blue"),
            RawWaypoint(name: "Plain", note: nil, coordinate: eastward[2].coordinate),
        ]
        let placed = WaypointPlacement.place(raw, along: eastward)
        #expect(placed.map(\.name) == ["Fountain", "Turn", "Plain"])
        #expect(placed[0].category == .water)
        #expect(placed[1].category == nil, "an unmapped symbol is generic, and the waypoint survives")
        #expect(placed[2].category == nil)
    }
}
