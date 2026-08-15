import Foundation

/// Stable identifier for a route **in the app's library** — app-generated,
/// never a device object id.
///
/// A thin `String` wrapper for type safety (a route id can't be passed where a
/// ride id is expected). Route identity is split across the BLE boundary
/// (#359): the device names its copies by ``DeviceObjectID``, and
/// `PlannedRouteRecord.deviceObjectID` is the app's durable link between the
/// two namespaces — a `RouteID` never crosses the transport's data plane.
public struct RouteID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// One entry of the device's route catalog (`listRoutes()` — the `routeList`
/// object, spec §7.4): the durable device object id plus the display fields.
/// Deliberately **not** a `RouteSummary` — the catalog is keyed by
/// ``DeviceObjectID``, and its one consumer (reconciling the C1 "on device"
/// badge, #289) compares those ids against `PlannedRouteRecord.deviceObjectID`;
/// it never feeds list rows.
public struct RouteCatalogEntry: Identifiable, Equatable, Sendable {
    public let id: DeviceObjectID
    public var name: String
    /// Route length in metres.
    public var distanceMeters: Double
    /// Total climb in metres.
    public var elevationGainMeters: Double
    /// Number of geometry points in the stored route object.
    public var pointCount: Int
    /// The stored object's whole-object CRC-32 (v2 `routeList` entry, spec §7.4) —
    /// the content fingerprint that lets the app verify *what* a linked id points
    /// at (identity-verified badges) and recognize an identical unlinked copy
    /// (adopt-by-content). `0` = unknown (the device hasn't filled the side-loaded
    /// sidecar yet, or a genuine CRC of `0`, read the same by spec — no
    /// special-casing). The badge/adoption logic that consumes it is V6 (#770).
    public var crc32: UInt32
    /// The device's computed auto-delete instant for this route (epic #638), from
    /// the v2+expiry `routeList` entry's `expires_at` tail (spec §7.4). `nil` when
    /// the device hasn't started the countdown (`last_used == 0`), retention is
    /// ``Retention/never``, **or** the device predates expiry (a pre-tail 76-byte
    /// entry — see ``RouteListEntry``). Display-only; goes stale gracefully as
    /// extend-on-use moves it.
    public var expiresAt: Date?
    /// The device's stored retention level for this route (epic #638), from the
    /// entry's `retention` tail byte. `nil` when the device predates expiry (no
    /// tail); a known byte decodes via ``Retention/init(safeRawValue:)``.
    public var retention: Retention?

    public init(
        id: DeviceObjectID,
        name: String,
        distanceMeters: Double,
        elevationGainMeters: Double,
        pointCount: Int = 0,
        crc32: UInt32 = 0,
        expiresAt: Date? = nil,
        retention: Retention? = nil
    ) {
        self.id = id
        self.name = name
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.pointCount = pointCount
        self.crc32 = crc32
        self.expiresAt = expiresAt
        self.retention = retention
    }
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

/// Everything the route-detail screen (E2) renders beyond the list summary:
/// the waypoints (W1) and the elevation-profile data. Served by
/// `DeviceObjects.routeDetail(_:)` — the wire mapping is provisional until
/// firmware `S0` pins the detail read (see `OBCProtocol.md`).
public struct RouteDetail: Equatable, Sendable {
    public var summary: RouteSummary
    /// Waypoints along the route, in ride order (W1).
    public var waypoints: [Waypoint]
    /// Elevation samples along the route in metres, evenly spaced start → end.
    /// Empty when the source carried no elevation.
    public var elevationProfile: [Double]
    /// Steepest sustained climb grade in percent (E2's MAX stat), when known.
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
    /// The device object id to **replace**, or `nil` for a fresh upload (the device
    /// assigns a new id). Set when re-uploading an edited route that's already on
    /// the device so it updates in place instead of duplicating — "uploading to an
    /// existing id replaces that object" (`obc-ble-interface-spec.md` §4.2).
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
