import Foundation

/// Stable identifier for a route object on the device / in the app library.
///
/// **B-S0 skeleton** — a thin `String` wrapper for type safety; `B1` keeps it.
public struct RouteID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// Where an imported route came from **as a wire format**. The phone converts
/// **both GPX and TCX** to the compact binary route format before upload — the
/// device never parses XML (see `OBCProtocol.md` → *Delta 2*). H5 rejects any
/// other file type.
///
/// **B-S0 skeleton** — `B1`/`B6` may add provenance (Komoot/Strava/Files); the
/// contract-level fact pinned here is the accepted format set.
public enum RouteSource: Equatable, Sendable {
    case gpx
    case tcx
}

/// Lightweight route metadata for list rows (C1) and detail headers (E1/E2) —
/// no geometry payload. The binary payload rides in `RouteBlob`.
///
/// **B-S0 skeleton** — `B1` finalizes the field set (waypoints, track-preview
/// geometry, est. time, …). Kept minimal so `B1` imports rather than invents it.
public struct RouteSummary: Identifiable, Equatable, Sendable {
    public let id: RouteID
    public var name: String
    /// Route length in metres.
    public var distanceMeters: Double
    /// Total climb in metres.
    public var elevationGainMeters: Double
    /// Import format lineage (`nil` for routes authored/planned on-device).
    public var source: RouteSource?

    public init(
        id: RouteID,
        name: String,
        distanceMeters: Double,
        elevationGainMeters: Double,
        source: RouteSource? = nil
    ) {
        self.id = id
        self.name = name
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.source = source
    }
}

/// A full route ready to upload: metadata + the **compact binary payload** the
/// device stores verbatim. `B1`'s `BLEChannel` produces `payload` from a
/// GPX/TCX import; the device never sees XML (see `OBCProtocol.md` → *Object
/// formats* / *Delta 2*).
///
/// **B-S0 skeleton** — `B1` owns the codec that fills `payload` and any waypoint
/// side-tables. This type only pins the metadata + opaque-bytes shape.
public struct RouteBlob: Equatable, Sendable {
    public let summary: RouteSummary
    /// Opaque compact-binary route bytes. **No codec here** — that is `B1`.
    public let payload: Data

    public init(summary: RouteSummary, payload: Data) {
        self.summary = summary
        self.payload = payload
    }
}
