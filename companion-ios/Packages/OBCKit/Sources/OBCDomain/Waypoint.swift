import Foundation

/// A point of interest along a route — what the waypoint list (W1) renders and
/// what rides back the route detail (E-series). Waypoints travel with a
/// `RouteBlob` (route object side-table), not as a separate wire object.
///
/// **B1 finalization** of the B-S0 domain set. Ordered by `index` along the
/// route; `distanceAlongMeters` is the cumulative distance from the start.
public struct Waypoint: Identifiable, Equatable, Sendable {
    /// Ordinal position along the route (0-based, monotonic). Doubles as `id`.
    public let index: Int
    public var name: String
    /// Optional free-text note (nil when the source carried none).
    public var note: String?
    /// Cumulative distance from the route start, in metres.
    public let distanceAlongMeters: Double
    public let coordinate: Coordinate
    /// What this waypoint *is*, mapped from the source file's `<sym>`/`<type>`
    /// (`WaypointSymbol`, in OBCFormats). `nil` = generic — both the common case
    /// (a hand-placed "turn left here") and the fallback for an unmapped symbol.
    public var category: WaypointCategory?
    /// Signed lateral distance from the route line, metres: **positive = right**
    /// of the direction of travel, negative = left, `0` = on-route. Fixed at
    /// import by the same projection that yields `distanceAlongMeters`
    /// (`OBCR_Spec.md` §4) and stored in the route object, because a riding
    /// device cannot re-measure it from the decimated geometry it keeps.
    public var lateralOffsetMeters: Double
    public var provenance: WaypointProvenance?

    public var id: Int { index }

    public init(
        index: Int,
        name: String,
        note: String? = nil,
        distanceAlongMeters: Double,
        coordinate: Coordinate,
        category: WaypointCategory? = nil,
        lateralOffsetMeters: Double = 0,
        provenance: WaypointProvenance? = nil
    ) {
        self.index = index
        self.name = name
        self.note = note
        self.distanceAlongMeters = distanceAlongMeters
        self.coordinate = coordinate
        self.category = category
        self.lateralOffsetMeters = lateralOffsetMeters
        self.provenance = provenance
    }
}

public struct WaypointProvenance: Codable, Equatable, Sendable {
    public let store: Data
    public let object: UInt64
    public let revision: UInt64
    public let ordinal: UInt16
    public init(store: Data, object: UInt64, revision: UInt64, ordinal: UInt16) {
        self.store = store; self.object = object; self.revision = revision; self.ordinal = ordinal
    }
}
