import Foundation

/// Stable identifier for a tracked ride on the device / in the app library.
///
/// **B-S0 skeleton** — a thin `String` wrapper for type safety; `B1` keeps it.
public struct RideID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// Lightweight metadata for a device-recorded ride — the Tracked-tab row (C2).
/// Rides download over the CoC data plane as compact binary; this is just the
/// enumerable summary the `RideList` characteristic exposes.
///
/// **B-S0 skeleton** — `B1` finalizes the field set (moving time, avg speed,
/// climb, track-preview geometry). Kept minimal so `B1` imports it.
public struct RideSummary: Identifiable, Equatable, Sendable {
    public let id: RideID
    public var name: String
    /// Ride start time.
    public var date: Date
    /// Distance covered, in metres.
    public var distanceMeters: Double

    public init(id: RideID, name: String, date: Date, distanceMeters: Double) {
        self.id = id
        self.name = name
        self.date = date
        self.distanceMeters = distanceMeters
    }
}
