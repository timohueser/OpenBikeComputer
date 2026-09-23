import Foundation

/// The exact finalized device object from which a canonical ride was decoded.
/// This identifies content; it is not proof of a durable client or device write.
public struct RideSource: Codable, Hashable, Sendable {
    public let storeID: String
    public let objectID: UInt64
    public let revision: UInt64
    public let payloadLength: UInt64
    public let payloadCRC32: UInt32

    public init(storeID: String, objectID: UInt64, revision: UInt64,
                payloadLength: UInt64, payloadCRC32: UInt32) {
        self.storeID = storeID
        self.objectID = objectID
        self.revision = revision
        self.payloadLength = payloadLength
        self.payloadCRC32 = payloadCRC32
    }

    public func matches(_ id: RideID) -> Bool {
        revision != 0 && objectID != 0 && id.scope?.storeID == storeID
            && id.deviceObjectID?.raw == objectID
    }
}

/// A ride's raw library key. Device keys contain the full StoreId, object ID,
/// and serial. Other strings remain valid archive keys without a device scope.
public struct RideID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }

    private static let v4Prefix = "v4:"

    /// A ride id in one device's current-era namespace: what the transport mints from the device's
    /// ride catalog once the identity read has established the scope, and what the library stores.
    public init(deviceObjectID: DeviceObjectID, scope: LibraryScope) {
        self.init("\(Self.v4Prefix)\(scope.storeID):\(deviceObjectID.raw):\(scope.serial)")
    }

    /// An unscoped ride id for stand-ins with no device identity. The real transport always mints
    /// scoped ids.
    public init(deviceObjectID: DeviceObjectID) {
        self.init(String(deviceObjectID.raw))
    }

    /// The current device scope, or nil for an arbitrary archival key.
    public var scope: LibraryScope? {
        guard let components = scopedComponents else { return nil }
        return LibraryScope(serial: components.serial, storeID: components.storeID)
    }

    /// The device object id behind this ride id, parsed from either shape, or nil for an id that
    /// never came from a device catalog.
    public var deviceObjectID: DeviceObjectID? {
        if let raw = UInt64(rawValue) { return DeviceObjectID(raw) }
        return scopedComponents?.objectID
    }

    private var scopedComponents: (storeID: String, objectID: DeviceObjectID, serial: String)? {
        let parts = rawValue.split(separator: ":", maxSplits: 3, omittingEmptySubsequences: false)
        guard parts.count == 4, parts[0] == "v4", parts[1].count == 32,
            parts[1].utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }),
            let objectID = UInt64(parts[2])
        else { return nil }
        return (String(parts[1]), DeviceObjectID(objectID), String(parts[3]))
    }
}

/// Metadata for a device-recorded ride: the Tracked row and the sync list. Rides download as
/// compact binary, and this is the enumerable summary the device's ride catalog exposes.
/// One tracklog sample of a recorded ride.
public struct RidePoint: Hashable, Sendable {
    public let timestamp: Date
    public let coordinate: Coordinate
    /// Elevation in metres, when the device recorded one.
    public let elevationMeters: Double?
    /// Heart rate at this fix, when a strap was reporting fresh data, and nil when it was absent
    /// or stale. Independent of the elevation: a point can carry sensors without one.
    public let heartRate: Int?
    /// Crank cadence (rpm) at this fix, or `nil` when absent/stale.
    public let cadence: Int?
    /// Power (W) at this fix, or `nil` when absent/stale.
    public let power: Int?
    /// Whether this sample starts a new recorded track segment.
    public var segmentStart: Bool = false

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

    public init(
        timestamp: Date,
        coordinate: Coordinate,
        elevationMeters: Double? = nil,
        heartRate: Int? = nil,
        cadence: Int? = nil,
        power: Int? = nil,
        segmentStart: Bool
    ) {
        self.init(timestamp: timestamp, coordinate: coordinate, elevationMeters: elevationMeters,
                  heartRate: heartRate, cadence: cadence, power: power)
        self.segmentStart = segmentStart
    }
}

/// A full tracked ride: the canonical in-app model. The device ride codec decodes into this, and
/// every export format encodes from it, so a tracked-file format switch never touches storage,
/// sync or the screens.
public struct Ride: Identifiable, Equatable, Sendable {
    public var summary: RideSummary
    public var points: [RidePoint]

    public var id: RideID { summary.id }

    public init(summary: RideSummary, points: [RidePoint]) {
        self.summary = summary
        self.points = points
    }

    /// This ride as a planned route under the ride's name: the tracked line with its elevation,
    /// without time or sensors. It is not simplified here, because the route codec decimates
    /// every route at upload, so a ride keeps the density of an imported GPX track.
    ///
    /// A route is one line, so it joins the ride's segments with a straight leg at each break.
    /// The device does not count a break's jump as ridden, but the route counts it and navigates
    /// it. Nil when those legs would add more than 1 % to the ridden distance.
    public func plannedRoute() -> ImportedRoute? {
        var ridden = 0.0
        var bridged = 0.0
        for (from, to) in zip(points, points.dropFirst()) {
            let leg = from.coordinate.routeDistance(to: to.coordinate)
            if to.segmentStart { bridged += leg } else { ridden += leg }
        }
        guard bridged <= ridden * 0.01 else { return nil }
        return ImportedRoute(
            name: summary.name,
            points: points.map { RoutePoint(coordinate: $0.coordinate, elevationMeters: $0.elevationMeters) }
        )
    }
}

/// Everything the ride-detail screen renders beyond the list summary.
public struct RideDetail: Equatable, Sendable {
    public var summary: RideSummary
    /// Elevation samples along the ride in metres, evenly spaced from start to end. Empty when the
    /// tracklog carried no elevation.
    public var elevationProfile: [Double]

    public init(summary: RideSummary, elevationProfile: [Double] = []) {
        self.summary = summary
        self.elevationProfile = elevationProfile
    }
}

public struct RideSummary: Identifiable, Equatable, Sendable {
    public var source: RideSource?
    public let id: RideID
    public var name: String
    /// Ride start time.
    public var date: Date
    /// Distance covered, in metres.
    public var distanceMeters: Double
    /// Moving time, which excludes stops, in seconds.
    public var movingTime: TimeInterval
    /// Average moving speed, in metres per second.
    public var averageSpeedMps: Double
    /// Total climb, in metres.
    public var climbMeters: Double
    /// Normalized polyline for the preview component. Nil until geometry is decoded.
    public var trackPreview: TrackPreview?

    /// Per-ride sensor summary. Each is nil when the ride saw no fresh sample of that quantity,
    /// and the ride-detail screen shows a row only for the ones present.
    public var avgHeartRate: Int?
    public var maxHeartRate: Int?
    public var avgCadence: Int?
    public var avgPower: Int?
    public var maxPower: Int?

    /// The bike type that was current when the ride started. The rider can change it on the phone;
    /// the device copy does not change.
    public var bikeType: BikeType
    /// The trip day the ride started on, or nil.
    public var trip: RideTrip?

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
        maxPower: Int? = nil,
        bikeType: BikeType = .road,
        trip: RideTrip? = nil,
        source: RideSource? = nil
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
        self.bikeType = bikeType
        self.trip = trip
        self.source = source
    }
}

/// The trip day a ride started on (`specs/obc-ble-interface-spec.md` §7.2).
public struct RideTrip: Equatable, Sendable {
    /// The trip key of the trip object; never 0.
    public var key: UInt64
    /// 0-based; the rider sees Day 1 for index 0.
    public var dayIndex: Int
    /// The trip's day count when the ride started.
    public var dayCount: Int
    /// The trip's name when the ride was saved. It stays after the trip is deleted, and it is empty
    /// when the device no longer held the trip.
    public var name: String

    public init(key: UInt64, dayIndex: Int, dayCount: Int, name: String) {
        self.key = key
        self.dayIndex = dayIndex
        self.dayCount = dayCount
        self.name = name
    }
}

/// The device's tracked-ride catalog: the enumerable summaries plus the bounded catalog's
/// truncation signal.
///
/// Past the device's cap the catalog scan drops the excess in arbitrary order, so the header
/// states the full total and the list is truncated when that total exceeds the count. Some rides
/// are then silently unsyncable until the rider frees space, and this is the only honest signal on
/// the wire. The app surfaces a one-line warning instead of answering "up to date".
public struct RideCatalog: Equatable, Sendable {
    /// The rides the list carried.
    public var rides: [RideSummary]
    /// How many rides the device holds beyond what the list carried; zero when the whole catalog
    /// fit.
    public var hiddenRideCount: Int

    /// Whether the device dropped rides past its cap: the warning trigger.
    public var isTruncated: Bool { hiddenRideCount > 0 }

    public init(rides: [RideSummary], hiddenRideCount: Int = 0) {
        self.rides = rides
        self.hiddenRideCount = max(0, hiddenRideCount)
    }
}
