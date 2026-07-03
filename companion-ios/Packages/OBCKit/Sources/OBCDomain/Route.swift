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
/// device never parses XML.
public enum RouteSource: Equatable, Sendable {
    case gpx
    case tcx
}

/// Lightweight route metadata for list rows and detail headers — no geometry
/// payload beyond the normalized `trackPreview`. The full binary payload rides
/// in `RouteBlob`.
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
    /// Normalized polyline for the `GPSTrackPreview`. `nil` until geometry is
    /// decoded; `.empty` for a genuinely empty track.
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

/// Everything the route-detail screen renders beyond the list summary: the
/// waypoints and the elevation-profile data. Served by
/// `DeviceTransport.routeDetail(_:)`.
public struct RouteDetail: Equatable, Sendable {
    public var summary: RouteSummary
    /// Waypoints along the route, in ride order.
    public var waypoints: [Waypoint]
    /// Elevation samples along the route in metres, evenly spaced start → end.
    /// Empty when the source carried no elevation.
    public var elevationProfile: [Double]
    /// Steepest sustained climb grade in percent, when known.
    public var maxGradePercent: Double?

    public init(
        summary: RouteSummary,
        waypoints: [Waypoint] = [],
        elevationProfile: [Double] = [],
        maxGradePercent: Double? = nil
    ) {
        self.summary = summary
        self.waypoints = waypoints
        self.elevationProfile = elevationProfile
        self.maxGradePercent = maxGradePercent
    }
}

/// A full route ready to upload: metadata + waypoints + the **compact binary
/// payload** the device stores verbatim. `BLEChannel` frames `payload` over the
/// CoC data plane; the import path produces it from GPX/TCX — the device never
/// sees XML.
///
/// The `payload` stays **opaque bytes** at this layer: its internal byte layout is
/// owned by the on-device route format, and `BLEChannel` moves it without
/// interpreting it.
public struct RouteBlob: Equatable, Sendable {
    public let summary: RouteSummary
    /// Waypoints along the route. Travels with the route, not as a separate
    /// wire object.
    public let waypoints: [Waypoint]
    /// Opaque compact-binary route bytes — framed, not parsed, by `BLEChannel`.
    public let payload: Data
    /// The device object id to **replace**, or `nil` for a fresh upload (the device
    /// assigns a new id). Set when re-uploading an edited route that's already on
    /// the device so it updates in place instead of duplicating — uploading to an
    /// existing id replaces that object.
    public let targetObjectID: UInt16?

    public init(
        summary: RouteSummary, waypoints: [Waypoint] = [], payload: Data,
        targetObjectID: UInt16? = nil
    ) {
        self.summary = summary
        self.waypoints = waypoints
        self.payload = payload
        self.targetObjectID = targetObjectID
    }
}
