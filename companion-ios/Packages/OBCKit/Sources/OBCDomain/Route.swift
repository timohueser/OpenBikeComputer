import Foundation

/// Stable identifier for a route object on the device / in the app library.
///
/// A thin `String` wrapper for type safety (a route id can't be passed where a
/// ride id is expected).
public struct RouteID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// Where an imported route came from **as a wire format**. The phone converts
/// **both GPX and TCX** to the compact binary route format before upload — the
/// device never parses XML (see `OBCProtocol.md` → *Delta 2*). H5 rejects any
/// other file type.
public enum RouteSource: Equatable, Sendable {
    case gpx
    case tcx
}

/// Lightweight route metadata for list rows (C1) and detail headers (E1/E2) —
/// no geometry payload beyond the normalized `trackPreview`. The full binary
/// payload rides in `RouteBlob`.
///
/// **B1 finalization** of the B-S0 skeleton: adds the estimated duration, point
/// count, and the `TrackPreview` the list/detail render. New fields are defaulted
/// so the B-S0 call sites keep compiling.
public struct RouteSummary: Identifiable, Equatable, Sendable {
    public let id: RouteID
    public var name: String
    /// Route length in metres.
    public var distanceMeters: Double
    /// Total climb in metres.
    public var elevationGainMeters: Double
    /// Estimated ride time, if the source/device provides one (`nil` otherwise).
    public var estimatedDuration: TimeInterval?
    /// Number of geometry points in the full route (the preview may be downsampled).
    public var pointCount: Int
    /// Import format lineage (`nil` for routes authored/planned on-device).
    public var source: RouteSource?
    /// Normalized polyline for the `GPSTrackPreview` (B11). `nil` until geometry
    /// is decoded; `.empty` for a genuinely empty track.
    public var trackPreview: TrackPreview?

    public init(
        id: RouteID,
        name: String,
        distanceMeters: Double,
        elevationGainMeters: Double,
        estimatedDuration: TimeInterval? = nil,
        pointCount: Int = 0,
        source: RouteSource? = nil,
        trackPreview: TrackPreview? = nil
    ) {
        self.id = id
        self.name = name
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.estimatedDuration = estimatedDuration
        self.pointCount = pointCount
        self.source = source
        self.trackPreview = trackPreview
    }
}

/// A full route ready to upload: metadata + waypoints + the **compact binary
/// payload** the device stores verbatim. `B1`'s `BLEChannel` frames `payload`
/// over the CoC data plane; the import path (B6) produces it from GPX/TCX — the
/// device never sees XML (see `OBCProtocol.md` → *Object formats* / *Delta 2*).
///
/// The `payload` stays **opaque bytes** at this layer: its internal byte layout is
/// owned by firmware `S0` + the on-device route format, and `BLEChannel` moves it
/// without interpreting it.
public struct RouteBlob: Equatable, Sendable {
    public let summary: RouteSummary
    /// Waypoints along the route (W1). Travels with the route, not as a separate
    /// wire object.
    public let waypoints: [Waypoint]
    /// Opaque compact-binary route bytes — framed, not parsed, by `BLEChannel`.
    public let payload: Data

    public init(summary: RouteSummary, waypoints: [Waypoint] = [], payload: Data) {
        self.summary = summary
        self.waypoints = waypoints
        self.payload = payload
    }
}
