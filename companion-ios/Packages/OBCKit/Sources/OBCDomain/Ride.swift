import Foundation

/// Stable identifier for a tracked ride on the device / in the app library.
///
/// A thin `String` wrapper for type safety (distinct from `RouteID`).
public struct RideID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// Metadata for a device-recorded ride — the Tracked-tab row (C2) and sync list.
/// Rides download over the CoC data plane as compact binary; this is the
/// enumerable summary the `RideList` characteristic exposes.
///
/// **B1 finalization** of the B-S0 skeleton: adds moving time, average speed,
/// climb, and the `TrackPreview`. New fields are defaulted so the B-S0 call sites
/// keep compiling.
public struct RideSummary: Identifiable, Equatable, Sendable {
    public let id: RideID
    public var name: String
    /// Ride start time.
    public var date: Date
    /// Distance covered, in metres.
    public var distanceMeters: Double
    /// Moving time (excludes stops), in seconds.
    public var movingTime: TimeInterval
    /// Average moving speed, in metres per second.
    public var averageSpeedMps: Double
    /// Total climb, in metres.
    public var climbMeters: Double
    /// Normalized polyline for the `GPSTrackPreview` (B11). `nil` until geometry
    /// is decoded.
    public var trackPreview: TrackPreview?

    public init(
        id: RideID,
        name: String,
        date: Date,
        distanceMeters: Double,
        movingTime: TimeInterval = 0,
        averageSpeedMps: Double = 0,
        climbMeters: Double = 0,
        trackPreview: TrackPreview? = nil
    ) {
        self.id = id
        self.name = name
        self.date = date
        self.distanceMeters = distanceMeters
        self.movingTime = movingTime
        self.averageSpeedMps = averageSpeedMps
        self.climbMeters = climbMeters
        self.trackPreview = trackPreview
    }
}
