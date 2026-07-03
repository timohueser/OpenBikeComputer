import Foundation

/// The device's copy of a planned route, as the phone can prove it: drives the
/// C1 badge (check / out-of-date) and the detail's Upload ↔ Update ↔ disabled
/// button. "Up to date" means the current upload payload's CRC-32 equals the
/// one the device committed — the same whole-object CRC the transfer verified.
public enum OnDeviceState: Equatable, Sendable {
    /// The device holds no copy (never uploaded, or deleted device-side).
    case notOnDevice
    /// The device's copy is byte-identical to what an upload would send now.
    case upToDate
    /// The device holds this route, but the phone's version has moved on
    /// (re-import, rename) — an upload replaces the copy in place.
    case outdated

    /// The one place the rule lives (the list model and the detail model both
    /// call it). `currentCRC` is a closure so the payload is only encoded when
    /// the comparison actually needs it. A device copy with an *unknown*
    /// fingerprint (a pre-fingerprint library) reads as outdated — offering an
    /// update self-heals it; calling it up-to-date would disable Upload with
    /// no way out.
    public static func determine(
        deviceObjectID: UInt16?,
        uploadedCRC32: UInt32?,
        currentCRC: () -> UInt32
    ) -> OnDeviceState {
        guard deviceObjectID != nil else { return .notOnDevice }
        guard let uploadedCRC32 else { return .outdated }
        return currentCRC() == uploadedCRC32 ? .upToDate : .outdated
    }
}

/// A planned route as the **phone's library** keeps it (B1S): the canonical
/// parsed `ImportedRoute`, the list `summary` derived from it at save time, the
/// original file bytes for re-parse/debugging, and whether the device has a
/// copy. This — not the device wire blob — is the long-term format: the
/// `RouteBlob` payload is firmware-`S0`-owned and gets re-encoded from `route`
/// whenever it's needed (see issue #256's format rule).
public struct PlannedRouteRecord: Identifiable, Equatable, Sendable {
    /// The list-row summary (C1) under the route's library id. Carries the
    /// display name — renames (H12) land here, never in `route`.
    public var summary: RouteSummary
    /// The canonical parsed model (geometry + waypoints), exactly as the
    /// import decoder produced it.
    public var route: ImportedRoute
    /// The original interchange file, byte-exact ("Schwarzwald.gpx" + bytes).
    public var sourceFileName: String
    public var sourceFileData: Data
    /// The device object id this route was assigned on upload — the durable link
    /// between a library route and its copy on the device (names/local ids can't
    /// match across the BLE boundary). `nil` until an upload commits (an
    /// H4 save-before-pairing import, or a route never pushed); a device-side
    /// delete clears it again at reconcile.
    public var deviceObjectID: UInt16?
    /// The CRC-32 of the upload payload the device last **committed** — the
    /// fingerprint behind ``OnDeviceState``. Set alongside ``deviceObjectID``
    /// when an upload's result lands; `nil` when the copy's content is unknown
    /// (pre-fingerprint library), which reads as outdated until the next push.
    public var uploadedCRC32: UInt32?
    /// When the route entered the library — newest-first list order.
    public var addedAt: Date

    public var id: RouteID { summary.id }

    /// Whether the device holds a copy — derived from ``deviceObjectID``.
    public var uploadedToDevice: Bool { deviceObjectID != nil }

    public init(
        summary: RouteSummary,
        route: ImportedRoute,
        sourceFileName: String,
        sourceFileData: Data,
        deviceObjectID: UInt16? = nil,
        uploadedCRC32: UInt32? = nil,
        addedAt: Date = Date()
    ) {
        self.summary = summary
        self.route = route
        self.sourceFileName = sourceFileName
        self.sourceFileData = sourceFileData
        self.deviceObjectID = deviceObjectID
        self.uploadedCRC32 = uploadedCRC32
        self.addedAt = addedAt
    }

    /// The detail screen's data (E2 for a phone-side route), derived from the
    /// canonical geometry — the device never had this route, so `routeDetail`
    /// can't answer for it. Same `RouteStats` path the import landing used, so
    /// reopening shows exactly what was saved.
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
