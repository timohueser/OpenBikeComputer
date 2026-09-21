import Foundation

/// Stable identifier for a trip in the app's library: app-generated, never a device object id.
///
/// A thin `String` wrapper for type safety, the exact ``RouteID`` idiom. A trip groups routes the
/// phone owns, and its device copy is named by a ``DeviceObjectID``, so a `TripID` never crosses
/// the transport's data plane.
public struct TripID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// A trip as the phone's library keeps it: a tiny metadata object that references planned routes
/// by ``RouteID`` in ride order. It never carries route payloads, so membership edits never touch
/// a route's bytes.
///
/// Ordering lives here and nowhere else: `stageIDs` is the single source of truth for stage order,
/// and the device object and every derived view read it in that order. A route belongs to at most
/// one trip, or is standalone at top level.
public struct TripRecord: Identifiable, Equatable, Sendable {
    public var id: TripID
    /// Display name. The codec truncates it on a character boundary at encode.
    public var name: String
    /// Member routes in ride order: the ordering source of truth. Each is a ``RouteID`` in the
    /// phone's library, and the trip references them and never the route bytes. The store keeps
    /// each id in at most one trip and drops any whose route record is gone.
    public var stageIDs: [RouteID]
    /// The device copy this trip was assigned on upload: the durable link between a library trip
    /// and its copy on one device in one id era, the same link routes use. A bare object id would
    /// silently match every connected device, and trip ids come from the device's own per-store
    /// counter. Nil until an upload commits, and a device-side delete clears it again at reconcile.
    /// Only meaningful when ``DeviceRouteLink/matches(_:)`` holds for the connected device.
    public var deviceLink: DeviceRouteLink?
    /// The CRC-32 of the trip object the device last committed: the fingerprint behind the
    /// trip-level ``OnDeviceState``. Set alongside ``deviceLink`` when an upload's result lands,
    /// and nil when the copy's content is unknown, which reads as outdated until the next push. A
    /// stage reorder changes this CRC while leaving the length and name untouched, so it is the
    /// only signal that detects an outdated trip.
    public var uploadedCRC32: UInt32?
    /// When the trip entered the library, which is the newest-first list order.
    public var addedAt: Date

    /// Whether some device holds a copy, derived from ``deviceLink``.
    public var uploadedToDevice: Bool { deviceLink != nil }

    public init(
        id: TripID,
        name: String,
        stageIDs: [RouteID],
        deviceLink: DeviceRouteLink? = nil,
        uploadedCRC32: UInt32? = nil,
        addedAt: Date = Date()
    ) {
        self.id = id
        self.name = name
        self.stageIDs = stageIDs
        self.deviceLink = deviceLink
        self.uploadedCRC32 = uploadedCRC32
        self.addedAt = addedAt
    }

    /// The trip-level ``OnDeviceState``, the same rule routes use. `currentCRC` is a closure, so
    /// the trip object is only encoded when the up-to-date against outdated split actually needs
    /// it; the encode lives in `OBCTransport`.
    public func onDeviceState(
        provenCommittedCRC: UInt32?,
        currentCRC: () -> UInt32
    ) -> OnDeviceState {
        OnDeviceState.determine(provenCommittedCRC: provenCommittedCRC, currentCRC: currentCRC)
    }
}

/// A trip's derived statistics: distance and climb summed over its member routes, plus the stage
/// count. The one implementation everything reads, so the phone and the device can never disagree
/// about a trip's numbers.
///
/// It sums over the summaries of the resolvable stages the caller hands it, so a dangling member
/// is simply not in the list, and `stageCount` is the count of what was summed.
public struct TripStats: Equatable, Sendable {
    public var distanceMeters: Double
    public var elevationGainMeters: Double
    public var stageCount: Int

    public init(distanceMeters: Double = 0, elevationGainMeters: Double = 0, stageCount: Int = 0) {
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.stageCount = stageCount
    }

    /// Sum distance and ascent and count the stages over the member summaries in the order given.
    /// The caller resolves the stage ids to summaries and keeps ride order.
    public static func summing<S: Sequence>(_ stages: S) -> TripStats where S.Element == RouteSummary {
        var stats = TripStats()
        for stage in stages {
            stats.distanceMeters += stage.distanceMeters
            stats.elevationGainMeters += stage.elevationGainMeters
            stats.stageCount += 1
        }
        return stats
    }
}

/// One entry of the device's trip catalog: the durable trip object id plus the summed display
/// fields the device computed over its resolvable stages. Deliberately not a `TripRecord`: the
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
