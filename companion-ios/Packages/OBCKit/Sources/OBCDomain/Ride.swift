import Foundation

/// Stable identifier for a tracked ride on the device / in the app library.
///
/// A thin `String` wrapper for type safety (distinct from `RouteID`).
///
/// Unlike routes, ride identity is **deliberately shared** across the BLE
/// boundary: the app reuses the device's durable ride id (spec §4.1, made
/// durable by the #289/#290 identity rework) as the library id, so the
/// synced/deleted tombstone sets key on it directly. The typed accessors below
/// make that device-namespace nature explicit — the real transport **mints**
/// ride ids via ``init(deviceObjectID:scope:)`` and reads them back via
/// ``deviceObjectID``, never by ad-hoc string↔int round-trips (#359).
///
/// **Composite keys (v2, #769):** a device object id is durable only within an
/// id era on one device, so the minted id string carries the whole
/// `(serial, epoch, id)` composite — `v2:<epoch>:<objectID>:<serial>` (serial
/// last, so a serial containing `:` needs no escaping; epoch and object id are
/// canonical decimal, making the encoding injective). Everything downstream —
/// the library's ride directories, the synced set, tombstones, trash marks —
/// keys on the raw string, so per-(serial, epoch) scoping *falls out of the
/// id*: an era change or a device switch changes the string, old keys stop
/// matching, and there is no re-key flow to tear. A bare-number id is a v1
/// (pre-scoping) legacy id — still readable, claimed by the one-time
/// migration when the device corroborates it, archival forever otherwise.
public struct RideID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }

    private static let scopedPrefix = "v2:"

    /// A ride id in one device's **current-era** namespace — what the
    /// transport mints from the device's ride catalog once the identity read
    /// has established the scope, and what the library then stores as-is.
    public init(deviceObjectID: DeviceObjectID, scope: LibraryScope) {
        self.init("\(Self.scopedPrefix)\(scope.epoch):\(deviceObjectID.raw):\(scope.serial)")
    }

    /// An **unscoped** ride id — the v1 shape (the bare device object id).
    /// Kept for the legacy-claim migration and for stand-ins that model no
    /// device identity; the real transport always mints scoped ids.
    public init(deviceObjectID: DeviceObjectID) {
        self.init(String(deviceObjectID.raw))
    }

    /// The `(serial, epoch)` scope this id was minted under, or `nil` for a
    /// v1 legacy id / an id that never came from a device catalog (mock
    /// fixtures, tests). `nil` is what keeps unclaimed flat entries archival:
    /// they can never equal a scoped key, and scope-filtered writes (the
    /// possession ack) skip them.
    public var scope: LibraryScope? {
        guard let (epoch, _, serial) = scopedComponents else { return nil }
        return LibraryScope(serial: serial, epoch: epoch)
    }

    /// The device object id behind this ride id — parsed from either shape —
    /// or `nil` for an id that never came from a device catalog.
    public var deviceObjectID: DeviceObjectID? {
        if let raw = UInt16(rawValue) { return DeviceObjectID(raw) }
        guard let (_, objectID, _) = scopedComponents else { return nil }
        return objectID
    }

    /// Decompose a `v2:<epoch>:<objectID>:<serial>` id; `nil` for any other
    /// shape. The serial is everything after the third `:` (it may itself
    /// contain `:`), and may be empty only in synthetic test scopes.
    private var scopedComponents: (epoch: UInt32, objectID: DeviceObjectID, serial: String)? {
        guard rawValue.hasPrefix(Self.scopedPrefix) else { return nil }
        let parts = rawValue.split(separator: ":", maxSplits: 3, omittingEmptySubsequences: false)
        guard parts.count == 4, let epoch = UInt32(parts[1]), let objectID = UInt16(parts[2])
        else { return nil }
        return (epoch, DeviceObjectID(objectID), String(parts[3]))
    }
}

/// Metadata for a device-recorded ride — the Tracked-tab row (C2) and sync list.
/// Rides download over the CoC data plane as compact binary; this is the
/// enumerable summary the `RideList` characteristic exposes.
///
/// **B1 finalization** of the B-S0 skeleton: adds moving time, average speed,
/// climb, and the `TrackPreview`. New fields are defaulted so the B-S0 call sites
/// keep compiling.
/// One tracklog sample of a recorded ride.
public struct RidePoint: Hashable, Sendable {
    public let timestamp: Date
    public let coordinate: Coordinate
    /// Elevation in metres, when the device recorded one.
    public let elevationMeters: Double?
    /// Heart rate (bpm) at this fix, when a strap was reporting fresh data —
    /// `nil` when absent/stale (ride object v2, epic #707). Independent of
    /// `elevationMeters`: a point can carry sensors without elevation.
    public let heartRate: Int?
    /// Crank cadence (rpm) at this fix, or `nil` when absent/stale.
    public let cadence: Int?
    /// Power (W) at this fix, or `nil` when absent/stale.
    public let power: Int?

    public init(
        timestamp: Date,
        coordinate: Coordinate,
        elevationMeters: Double? = nil,
        heartRate: Int? = nil,
        cadence: Int? = nil,
        power: Int? = nil
    ) {
        self.timestamp = timestamp
        self.coordinate = coordinate
        self.elevationMeters = elevationMeters
        self.heartRate = heartRate
        self.cadence = cadence
        self.power = power
    }
}

/// A full tracked ride — the **canonical in-app model**. The device ride codec
/// (compact binary, S0-owned) decodes into this, and every export format
/// (GPX today, FIT later, connected services) encodes *from* this via a
/// `RideFileEncoder` (see `OBCFormats`) — so a tracked-file format switch never
/// touches storage, sync, or the screens.
public struct Ride: Identifiable, Equatable, Sendable {
    public var summary: RideSummary
    public var points: [RidePoint]

    public var id: RideID { summary.id }

    public init(summary: RideSummary, points: [RidePoint]) {
        self.summary = summary
        self.points = points
    }
}

/// Everything the ride-detail screen (E3) renders beyond the list summary.
/// Served by `DeviceTransport.rideDetail(_:)`; like `RouteDetail`, the wire
/// mapping is provisional until firmware `S0` pins it.
public struct RideDetail: Equatable, Sendable {
    public var summary: RideSummary
    /// Elevation samples along the ride in metres, evenly spaced start → end.
    /// Empty when the tracklog carried no elevation.
    public var elevationProfile: [Double]

    public init(summary: RideSummary, elevationProfile: [Double] = []) {
        self.summary = summary
        self.elevationProfile = elevationProfile
    }
}

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

    /// Per-ride BLE-sensor summary (ride object v2, epic #707). Each is `nil`
    /// when the ride saw no fresh sample of that quantity — the ride-detail
    /// screen shows a row only for the ones present. Always `nil` for a v1 ride.
    public var avgHeartRate: Int?
    public var maxHeartRate: Int?
    public var avgCadence: Int?
    public var avgPower: Int?
    public var maxPower: Int?

    public init(
        id: RideID,
        name: String,
        date: Date,
        distanceMeters: Double,
        movingTime: TimeInterval = 0,
        averageSpeedMps: Double = 0,
        climbMeters: Double = 0,
        trackPreview: TrackPreview? = nil,
        avgHeartRate: Int? = nil,
        maxHeartRate: Int? = nil,
        avgCadence: Int? = nil,
        avgPower: Int? = nil,
        maxPower: Int? = nil
    ) {
        self.id = id
        self.name = name
        self.date = date
        self.distanceMeters = distanceMeters
        self.movingTime = movingTime
        self.averageSpeedMps = averageSpeedMps
        self.climbMeters = climbMeters
        self.trackPreview = trackPreview
        self.avgHeartRate = avgHeartRate
        self.maxHeartRate = maxHeartRate
        self.avgCadence = avgCadence
        self.avgPower = avgPower
        self.maxPower = maxPower
    }
}

/// The device's tracked-ride catalog (`listRides()` — the `rideList` object,
/// spec §7.4): the enumerable summaries plus the **truncation** signal the v2
/// list header carries.
///
/// Past the device's `MAX_RIDES` cap the catalog scan drops the excess in
/// FAT-arbitrary order, so the header states the full `total` and the list is
/// **truncated** when `total > count` — some rides are silently unsyncable until
/// the rider frees space, and this is the only honest signal on the wire (the
/// device-side cull order is arbitrary). The app surfaces a one-line warning
/// instead of answering "up to date".
public struct RideCatalog: Equatable, Sendable {
    /// The rides the list carried (the header `count`).
    public var rides: [RideSummary]
    /// How many rides the device holds **beyond** what the list carried
    /// (`total − count`); `0` when the whole catalog fit.
    public var hiddenRideCount: Int

    /// Whether the device dropped rides past its cap — the warning trigger.
    public var isTruncated: Bool { hiddenRideCount > 0 }

    public init(rides: [RideSummary], hiddenRideCount: Int = 0) {
        self.rides = rides
        self.hiddenRideCount = max(0, hiddenRideCount)
    }
}
