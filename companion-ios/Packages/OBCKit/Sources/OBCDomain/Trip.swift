import Foundation

/// Stable identifier for a **trip** in the app's library — app-generated, never
/// a device object id.
///
/// A thin `String` wrapper for type safety, the exact ``RouteID`` idiom: a trip
/// groups routes the phone owns, and its device copy is named by a
/// ``DeviceObjectID`` (via `TripRecord.deviceLink`) — a `TripID` never crosses
/// the transport's data plane.
public struct TripID: Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
}

/// A **trip** as the phone's library keeps it (TR5): a tiny metadata object that
/// *references* planned routes by ``RouteID`` in ride order — it never carries
/// route payloads (a route is a byte-identical OBCR file; membership edits never
/// touch it). The `PlannedRouteRecord` idiom: canonical value type, the
/// durable-link + fingerprint pair the device parity reads, and an `addedAt` for
/// newest-first list order.
///
/// **Ordering lives here and nowhere else** — `stageIDs` is the single source of
/// truth for stage order; the device object and every derived view read it in
/// this order. A route belongs to **at most one** trip (the `LibraryStore`
/// invariant), or is standalone at top level.
public struct TripRecord: Identifiable, Equatable, Sendable {
    public var id: TripID
    /// Display name (≤ 48 UTF-8 bytes on the wire; the codec truncates on a
    /// character boundary at encode).
    public var name: String
    /// Member routes in **ride order** — the ordering source of truth. Each is a
    /// ``RouteID`` in the phone's library; the trip references them and never the
    /// route bytes. The `LibraryStore` keeps each id in ≤ 1 trip and drops any
    /// whose route record is gone.
    public var stageIDs: [RouteID]
    /// The device copy this trip was assigned on upload — the durable
    /// `{serial, epoch, id}` link between a library trip and its copy on **one
    /// device in one id era**, the same ``DeviceRouteLink`` routes use (#769: a
    /// bare object id silently matched every connected device, and trip ids come
    /// from the device's own per-store counter). `nil` until an upload commits;
    /// a device-side delete clears it again at reconcile. Only meaningful when
    /// ``DeviceRouteLink/matches(_:)`` holds for the connected device — TR8's
    /// adoption rule ("push the trip iff it already exists on the device") and
    /// replace-by-id both consume it through that predicate.
    public var deviceLink: DeviceRouteLink?
    /// The CRC-32 of the trip object the device last **committed** — the
    /// fingerprint behind the trip-level ``OnDeviceState``. Set alongside
    /// ``deviceLink`` when an upload's result lands; `nil` when the copy's
    /// content is unknown (which reads as outdated until the next push). A stage
    /// reorder changes this CRC while leaving `byte_len`/`name` untouched, so it
    /// is the only signal that detects an outdated trip.
    public var uploadedCRC32: UInt32?
    /// When the trip entered the library — newest-first list order.
    public var addedAt: Date

    /// Whether some device holds a copy — derived from ``deviceLink``.
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

    /// The trip-level ``OnDeviceState`` — the same rule routes use, reusing
    /// ``OnDeviceState/determine(provenCommittedCRC:currentCRC:)``. `currentCRC`
    /// is a closure so the trip object is only encoded when the up-to-date /
    /// outdated split actually needs it (the encode lives in `OBCTransport`).
    public func onDeviceState(
        provenCommittedCRC: UInt32?,
        currentCRC: () -> UInt32
    ) -> OnDeviceState {
        OnDeviceState.determine(provenCommittedCRC: provenCommittedCRC, currentCRC: currentCRC)
    }
}

/// A trip's derived statistics: distance + climb summed over its member routes,
/// plus the stage count. **The one implementation** everything reads — the trip
/// card, the trip page, and the device-parity `tripList` totals all sum here, so
/// the phone and the device can never disagree about a trip's numbers.
///
/// Sums over the ``RouteSummary``s of the **resolvable** stages the caller
/// hands it (a dangling member — a route deleted individually — simply isn't in
/// the list); `stageCount` is therefore the count of what was summed.
public struct TripStats: Equatable, Sendable {
    public var distanceMeters: Double
    public var elevationGainMeters: Double
    public var stageCount: Int

    public init(distanceMeters: Double = 0, elevationGainMeters: Double = 0, stageCount: Int = 0) {
        self.distanceMeters = distanceMeters
        self.elevationGainMeters = elevationGainMeters
        self.stageCount = stageCount
    }

    /// Sum distance/ascent + count the stages over the member summaries **in the
    /// order given** (the caller resolves `TripRecord.stageIDs` to summaries and
    /// keeps ride order). The single stats definition.
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

/// One entry of the device's trip catalog (`tripList`, spec §7.4): the durable
/// trip object id plus the summed display fields the device computed over its
/// resolvable stages. Deliberately **not** a `TripRecord` — the catalog is keyed
/// by ``DeviceObjectID`` (per-connection reconcile state from the connected
/// device, so the bare id is unambiguous here) and its one consumer (reconciling
/// the trip's "on device" badge) compares those ids against
/// `TripRecord.deviceLink` for links whose scope matches the connection; it
/// never feeds list rows. The exact mirror of ``RouteCatalogEntry``.
public struct TripCatalogEntry: Identifiable, Equatable, Sendable {
    public let id: DeviceObjectID
    public var name: String
    /// Summed route length in metres (over the device's resolvable stages).
    public var distanceMeters: Double
    /// Summed climb in metres (over the device's resolvable stages).
    public var elevationGainMeters: Double
    /// Every stored stage the device counts, **dangling refs included** (spec
    /// §7.4) — so it can exceed the number of stages the totals summed over.
    public var stageCount: Int
    /// The stored trip object's whole-object CRC-32 (v2 `tripList` entry) — the
    /// content fingerprint that detects an outdated trip (a stage reorder changes
    /// neither `byte_len` nor `name`). `0` = unknown, read the same by spec (no
    /// special-casing).
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
