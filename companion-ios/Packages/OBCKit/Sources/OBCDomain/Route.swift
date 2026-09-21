import Foundation

/// Stable identifier for a route in the app's library: app-generated, never a device object id.
///
/// A thin `String` wrapper for type safety, so a route id cannot be passed where a ride id is
/// expected. Route identity is split across the BLE boundary: the device names its copies by
/// ``DeviceObjectID``, and a planned record's device link joins the two namespaces, so a `RouteID`
/// never crosses the transport's data plane.
public struct RouteID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// One entry of the device's route catalog: the durable device object id plus the display fields.
/// Deliberately not a `RouteSummary`, because the catalog is keyed by ``DeviceObjectID`` and its
/// one consumer reconciles the "on device" badge against a record's device link. It never feeds
/// list rows.
public struct RouteCatalogEntry: Identifiable, Equatable, Sendable {
    public let id: DeviceObjectID
    public var name: String
    /// Route length in metres.
    public var distanceMeters: Double
    /// Total climb in metres.
    public var elevationGainMeters: Double
    /// Number of geometry points in the stored route object.
    public var pointCount: Int
    /// The stored object's whole-object CRC-32 from the catalog: the content fingerprint that lets
    /// the app verify what a linked id points at, and recognize an identical unlinked copy for
    /// adoption. Zero means unknown, which covers both a device that has not filled the sidecar yet
    /// and a genuine CRC of zero; the spec reads them the same way.
    public var crc32: UInt32

    public init(
        id: DeviceObjectID,
        name: String,
        distanceMeters: Double,
        elevationGainMeters: Double,
        pointCount: Int = 0,
        crc32: UInt32 = 0,
    ) {
        self.id = id
        self.name = name
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.pointCount = pointCount
        self.crc32 = crc32
    }
}

/// Where an imported route came from, as a wire format. The phone converts both supported XML
/// formats to the compact binary route format before upload, because the device never parses XML.
/// Any other file type is rejected at import.
public enum RouteSource: Equatable, Sendable {
    case gpx
    case tcx
}

/// Lightweight route metadata for list rows and detail headers: no geometry payload beyond the
/// normalized `trackPreview`. The full binary payload rides in `RouteBlob`.
public struct RouteSummary: Identifiable, Equatable, Sendable {
    public let id: RouteID
    public var name: String
    /// Route length in metres.
    public var distanceMeters: Double
    /// Total climb in metres.
    public var elevationGainMeters: Double
    /// Estimated ride time, if the source or the device provides one.
    public var estimatedDuration: TimeInterval?
    /// Number of geometry points in the full route; the preview may be downsampled.
    public var pointCount: Int
    /// Import format lineage; nil for routes authored on the device.
    public var source: RouteSource?
    /// Normalized polyline for the preview component. Nil until geometry is decoded, and `.empty`
    /// for a genuinely empty track.
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

/// Everything the route-detail screen renders beyond the list summary: the waypoints and the
/// elevation-profile data.
public struct RouteDetail: Equatable, Sendable {
    public var summary: RouteSummary
    /// Waypoints along the route, in ride order.
    public var waypoints: [Waypoint]
    /// Elevation samples along the route in metres, evenly spaced from start to end. Empty when
    /// the source carried no elevation.
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

/// A full route ready to upload: metadata, waypoints, and the compact binary payload the device
/// stores verbatim. The import path produces the payload, because the device never sees XML.
///
/// The payload stays opaque bytes at this layer: its internal byte layout is owned by the firmware
/// and the on-device route format, and the channel moves it without interpreting it.
public struct RouteBlob: Equatable, Sendable {
    public let summary: RouteSummary
    /// Waypoints along the route. They travel with the route, not as a separate wire object.
    public let waypoints: [Waypoint]
    /// Opaque compact-binary route bytes: framed, not parsed, by the channel.
    public let payload: Data
    /// The device object id to replace, or nil for a fresh upload, where the device assigns a new
    /// id. Set when re-uploading an edited route the device already holds, so it updates in place
    /// instead of duplicating: uploading to an existing id replaces that object.
    public let targetObjectID: DeviceObjectID?

    public init(
        summary: RouteSummary, waypoints: [Waypoint] = [], payload: Data,
        targetObjectID: DeviceObjectID? = nil
    ) {
        self.summary = summary
        self.waypoints = waypoints
        self.payload = payload
        self.targetObjectID = targetObjectID
    }
}
