import Foundation

/// The device's copy of a planned route, as far as the phone can prove it. "Up to date"
/// means the CRC-32 of the current upload payload equals the CRC the device committed.
public enum OnDeviceState: Equatable, Sendable {
    /// No provable copy: never uploaded, deleted on the device, or unproven.
    case notOnDevice
    /// The device's copy is byte-identical to what an upload would send now.
    case upToDate
    /// The device holds this route, but the phone's version moved on. An upload replaces
    /// the copy in place.
    case outdated

    /// `provenCommittedCRC` is the CRC the device is proven to hold now. `nil` means
    /// unproven and gives no badge: presence alone, or a `crc32 = 0` catalog entry, proves
    /// nothing. `currentCRC` is a closure so the payload is encoded only when it is needed.
    public static func determine(
        provenCommittedCRC: UInt32?,
        currentCRC: () -> UInt32
    ) -> OnDeviceState {
        guard let provenCommittedCRC else { return .notOnDevice }
        return currentCRC() == provenCommittedCRC ? .upToDate : .outdated
    }
}

/// A planned route as the phone's library keeps it. This, not the device wire blob, is the
/// long-term format: the `RouteBlob` payload is re-encoded from `route` when it is needed.
public struct PlannedRouteRecord: Identifiable, Equatable, Sendable {
    /// The list-row summary. It carries the display name: renames land here, not in `route`.
    public var summary: RouteSummary
    /// The canonical parsed model, exactly as the import decoder produced it.
    public var route: ImportedRoute
    /// The original interchange file, byte-exact.
    public var sourceFileName: String
    public var sourceFileData: Data
    /// The durable `{serial, StoreId, id}` link to the copy on one device in one id era: a
    /// bare object id would match every device. `nil` until an upload commits; a device-side
    /// delete clears it at reconcile. Only meaningful when ``DeviceRouteLink/matches(_:)`` holds.
    public var deviceLink: DeviceRouteLink?
    /// The CRC-32 of the upload payload the device last committed: the fingerprint behind
    /// ``OnDeviceState``. `nil` when the copy's content is unknown, which reads as outdated.
    public var uploadedCRC32: UInt32?
    /// When the route entered the library. The list orders newest first.
    public var addedAt: Date

    public var id: RouteID { summary.id }

    public var uploadedToDevice: Bool { deviceLink != nil }

    public init(
        summary: RouteSummary,
        route: ImportedRoute,
        sourceFileName: String,
        sourceFileData: Data,
        deviceLink: DeviceRouteLink? = nil,
        uploadedCRC32: UInt32? = nil,
        addedAt: Date = Date()
    ) {
        self.summary = summary
        self.route = route
        self.sourceFileName = sourceFileName
        self.sourceFileData = sourceFileData
        self.deviceLink = deviceLink
        self.uploadedCRC32 = uploadedCRC32
        self.addedAt = addedAt
    }

    /// The detail screen's data, derived from the canonical geometry. The device may never
    /// have held this route, so `routeDetail` cannot answer for it.
    public func detail() -> RouteDetail {
        let stats = RouteStats.compute(from: route.points)
        return RouteDetail(
            summary: summary,
            waypoints: route.waypoints,
            elevationProfile: stats.elevationProfile,
            maxGradePercent: stats.maxGradePercent
        )
    }
}
