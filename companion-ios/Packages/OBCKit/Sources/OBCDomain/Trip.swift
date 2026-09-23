import Foundation

/// Stable identifier for a trip in the app's library: app-generated, never a device object id.
///
/// A thin `String` wrapper for type safety, the exact ``RouteID`` idiom. The device copy of a trip
/// is named by a ``DeviceObjectID`` and keyed by ``Trip/key``, so a `TripID` never crosses the
/// transport's data plane.
public struct TripID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// A place where a day of a trip ends. The place is the truth; ``distance`` is derived from it
/// by projection onto the line after every line change.
public struct DayEnd: Equatable, Sendable {
    public var coordinate: Coordinate
    /// The rider's or the geocoder's name for the place.
    public var name: String?
    /// The own name of the day that ends here: its route's or file's name, or the rider's. It
    /// travels with the day, not with the place. Nil reads as "Day N ‹place›".
    public var title: String?
    /// Metres along the trip line. The stored value is the hint for the next projection, which
    /// keeps a day end on its own leg of an out-and-back or a loop.
    public var distance: Double
    /// The stop the rider ended the day at. The day end sits on the line point nearest it.
    public var stop: Stop?
    /// How the day reaches a stop off the line. Nil ends the day on the line.
    public var stopRoute: StopRoute?
    /// How the rider travels on from here when the next day starts elsewhere. Phone-only: the
    /// device knows a transfer by its geometry alone.
    public var transfer: TransferKind?
    /// The name of the place where the next day starts, when that is across a transfer.
    public var resumeName: String?

    public init(
        coordinate: Coordinate, name: String? = nil, title: String? = nil, distance: Double, stop: Stop? = nil,
        stopRoute: StopRoute? = nil, transfer: TransferKind? = nil, resumeName: String? = nil
    ) {
        self.coordinate = coordinate
        self.name = name
        self.title = title
        self.distance = distance
        self.stop = stop
        self.stopRoute = stopRoute
        self.transfer = transfer
        self.resumeName = resumeName
    }
}

/// How a transfer between two days goes.
public enum TransferKind: String, CaseIterable, Sendable {
    case train, bus, ferry, car
}

/// The device copy of one day route: the same link and fingerprint a planned route keeps.
public struct TripDayCopy: Equatable, Sendable {
    public var link: DeviceRouteLink
    /// The CRC-32 of the day route payload the device committed. Nil reads as outdated.
    public var uploadedCRC32: UInt32?

    public init(link: DeviceRouteLink, uploadedCRC32: UInt32?) {
        self.link = link
        self.uploadedCRC32 = uploadedCRC32
    }
}

/// A trip as the phone's library keeps it: one line with day ends on it. The line owns the
/// geometry; a route file added to a trip becomes part of the line and keeps no link to its old
/// library record. The upload cuts the line into one route per day.
///
/// The line is made of pieces, as ``MeasuredLine`` reads it. A gap between two pieces sits only
/// at a day end: the next day starts where the next piece starts.
public struct Trip: Identifiable, Equatable, Sendable {
    public var id: TripID
    /// The key the device stores progress and rides under. A trip keeps it for life, and a
    /// reverse gives the trip a new one, so the progress of the old direction never ticks days
    /// of the new one.
    public internal(set) var key: UInt64
    public var name: String
    /// Written into every day route of an upload.
    public var bikeType: BikeType
    /// The date of Day 1, when the rider set one.
    public var startDay: CivilDay?
    public internal(set) var line: [RoutePoint]
    /// Indices into ``line`` that start a new piece. Ascending, never 0.
    public internal(set) var pieceStarts: [Int]
    /// One per day, in ride order. The last one sits at the end of the line.
    public internal(set) var dayEnds: [DayEnd]
    /// The waypoints of the route files the line was joined from.
    public internal(set) var waypoints: [Stop]
    /// The name of the place where the line starts. A reverse swaps it with the last day end's
    /// name, so no name is lost.
    public internal(set) var startName: String?
    /// The device copy of each day route, by day index. A missing or nil entry has no copy.
    public var dayCopies: [TripDayCopy?]
    /// The device copy of the trip object: the same link a planned route keeps. Only meaningful
    /// when ``DeviceRouteLink/matches(_:)`` holds for the connected device.
    public var deviceLink: DeviceRouteLink?
    /// The CRC-32 of the trip object the device last committed. Nil reads as outdated.
    public var uploadedCRC32: UInt32?
    /// The trip key of the trip object the device last committed. When it differs from ``key``,
    /// the device holds progress for the old direction under that object.
    public var uploadedKey: UInt64?
    /// When the trip entered the library, which is the newest-first list order.
    public var addedAt: Date
    /// When the rider last changed the trip. Import offers to add a file to the most recently
    /// edited trip.
    public var editedAt: Date

    public init(
        id: TripID,
        key: UInt64 = Trip.newKey(),
        name: String,
        bikeType: BikeType,
        startDay: CivilDay? = nil,
        line: [RoutePoint] = [],
        pieceStarts: [Int] = [],
        dayEnds: [DayEnd] = [],
        waypoints: [Stop] = [],
        startName: String? = nil,
        dayCopies: [TripDayCopy?] = [],
        deviceLink: DeviceRouteLink? = nil,
        uploadedCRC32: UInt32? = nil,
        uploadedKey: UInt64? = nil,
        addedAt: Date,
        editedAt: Date? = nil
    ) {
        self.id = id
        self.key = max(key, 1)
        self.name = name
        self.bikeType = bikeType
        self.startDay = startDay
        self.line = line
        self.pieceStarts = pieceStarts
        self.dayEnds = dayEnds
        self.waypoints = waypoints
        self.startName = startName
        self.dayCopies = dayCopies
        self.deviceLink = deviceLink
        self.uploadedCRC32 = uploadedCRC32
        self.uploadedKey = uploadedKey
        self.addedAt = addedAt
        self.editedAt = editedAt ?? addedAt
    }

    /// A fresh trip key. The device reads key 0 as "no trip".
    public static func newKey() -> UInt64 { UInt64.random(in: 1...UInt64.max) }

    public var dayCount: Int { dayEnds.count }

    /// Name one day. The name travels with the day through every line change. Nil or blank
    /// returns the day to "Day N ‹place›".
    public mutating func renameDay(_ day: Int, to title: String?) {
        guard dayEnds.indices.contains(day) else { return }
        dayEnds[day].title = Self.trimmed(title)
    }

    /// Name the place where a day ends.
    public mutating func namePlace(_ day: Int, to name: String?) {
        guard dayEnds.indices.contains(day) else { return }
        dayEnds[day].name = Self.trimmed(name)
    }

    static func trimmed(_ name: String?) -> String? {
        let trimmed = name?.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed?.isEmpty == false ? trimmed : nil
    }

    /// Whether some device holds a copy of the trip object.
    public var uploadedToDevice: Bool { deviceLink != nil }
}

/// A trip's totals, summed over its days in ride order.
public struct TripStats: Equatable, Sendable {
    public var distanceMeters: Double
    public var elevationGainMeters: Double
    public var dayCount: Int

    public init(distanceMeters: Double = 0, elevationGainMeters: Double = 0, dayCount: Int = 0) {
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.dayCount = dayCount
    }
}

/// One entry of the device's trip catalog: the durable trip object id plus the summed display
/// fields the device computed over its resolvable day routes. Deliberately not a `Trip`: the
/// catalog is per-connection reconcile state from the connected device, so the bare id is
/// unambiguous here, and its one consumer compares those ids against a trip's device link. It
/// never feeds list rows. The exact mirror of ``RouteCatalogEntry``.
public struct TripCatalogEntry: Identifiable, Equatable, Sendable {
    public let id: DeviceObjectID
    public var name: String
    /// Summed route length in metres, over the device's resolvable stages.
    public var distanceMeters: Double
    /// Summed climb in metres, over the device's resolvable stages.
    public var elevationGainMeters: Double
    /// Every stored stage the device counts, dangling refs included, so it can exceed the number
    /// of stages the totals summed over.
    public var stageCount: Int
    /// The stored trip object's whole-object CRC-32 from the catalog: the content fingerprint that
    /// detects an outdated trip, because a stage reorder changes neither the length nor the name.
    /// Zero means unknown, and is read the same way by the spec.
    public var crc32: UInt32

    public init(
        id: DeviceObjectID,
        name: String,
        distanceMeters: Double,
        elevationGainMeters: Double,
        stageCount: Int = 0,
        crc32: UInt32 = 0
    ) {
        self.id = id
        self.name = name
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.stageCount = stageCount
        self.crc32 = crc32
    }
}
