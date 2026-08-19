import Foundation

/// The device's copy of a planned route, as the phone can **prove** it: drives
/// the C1 badge (check / out-of-date) and the detail's Upload ↔ Update ↔ disabled
/// button. "Up to date" means the current upload payload's CRC-32 equals the
/// one the device committed — the same whole-object CRC the transfer verified.
public enum OnDeviceState: Equatable, Sendable {
    /// The device holds no *provable* copy — never uploaded, deleted
    /// device-side, or unproven (no scoped link, an unknown catalog CRC, or a
    /// CRC that disagrees with what we committed). V6 (#770): no badge without
    /// proof.
    case notOnDevice
    /// The device's copy is byte-identical to what an upload would send now.
    case upToDate
    /// The device holds this route, but the phone's version has moved on
    /// (re-import, rename) — an upload replaces the copy in place.
    case outdated

    /// The one place the rule lives (the list model and the detail model both
    /// call it). `provenCommittedCRC` is the CRC the device is **proven** to
    /// currently hold for this record — a valid scoped link plus a catalog
    /// entry whose non-zero CRC equals the record's committed fingerprint (or a
    /// just-completed upload's verified CRC). `nil` means unproven → **no
    /// badge** (V6 #770: presence alone is never a checkmark; a `crc32 = 0`
    /// entry proves nothing, and a mismatch drops the link before it reaches
    /// here). `currentCRC` is a closure so the payload is only encoded when the
    /// up-to-date/outdated split actually needs it.
    public static func determine(
        provenCommittedCRC: UInt32?,
        currentCRC: () -> UInt32
    ) -> OnDeviceState {
        guard let provenCommittedCRC else { return .notOnDevice }
        return currentCRC() == provenCommittedCRC ? .upToDate : .outdated
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
    /// The device copy this route was assigned on upload — the durable
    /// `{serial, epoch, id}` link between a library route and its copy on
    /// **one device in one id era** (#769; names/local ids can't match across
    /// the BLE boundary, and a bare object id silently matched every device).
    /// `nil` until an upload commits (an H4 save-before-pairing import, a
    /// route never pushed, or a v1 flat link awaiting V6's CRC adoption); a
    /// device-side delete clears it again at reconcile. Only meaningful when
    /// ``DeviceRouteLink/matches(_:)`` holds for the connected device.
    public var deviceLink: DeviceRouteLink?
    /// The CRC-32 of the upload payload the device last **committed** — the
    /// fingerprint behind ``OnDeviceState``. Set alongside ``deviceLink``
    /// when an upload's result lands; `nil` when the copy's content is unknown
    /// (pre-fingerprint library), which reads as outdated until the next push.
    public var uploadedCRC32: UInt32?
    /// The **desired** app-side retention for this route (epic #638) — what the
    /// app pushes via `setRouteRetention` at upload and reconcile. **`nil` means
    /// "not set"** and pushes *nothing* (invariant 6: a route uploaded before this
    /// feature existed must never surprise-delete — it migrates as nil → the device
    /// keeps its `Never` default). An upload opts a `nil` record into
    /// ``Retention/appDefault`` (see the upload push). Distinct from the device's
    /// reported level below.
    public var retention: Retention?
    /// Device truth from the last protocol-v4 catalog reconcile: when the device will
    /// auto-delete this route (`nil` = never / not started / pre-expiry firmware).
    /// **Display-only** — it goes stale gracefully (extend-on-use moves it), so S7
    /// shows day granularity, not a live countdown.
    public var deviceExpiresAt: Date?
    /// Device truth from the last reconcile: the retention level the device
    /// currently stores for this route (`nil` = unknown / pre-expiry firmware).
    /// The reconcile compares it against ``retention`` to decide whether to push.
    public var deviceRetention: Retention?
    /// When the route entered the library — newest-first list order.
    public var addedAt: Date

    public var id: RouteID { summary.id }

    /// Whether some device holds a copy — derived from ``deviceLink``.
    public var uploadedToDevice: Bool { deviceLink != nil }

    public init(
        summary: RouteSummary,
        route: ImportedRoute,
        sourceFileName: String,
        sourceFileData: Data,
        deviceLink: DeviceRouteLink? = nil,
        uploadedCRC32: UInt32? = nil,
        retention: Retention? = nil,
        deviceExpiresAt: Date? = nil,
        deviceRetention: Retention? = nil,
        addedAt: Date = Date()
    ) {
        self.summary = summary
        self.route = route
        self.sourceFileName = sourceFileName
        self.sourceFileData = sourceFileData
        self.deviceLink = deviceLink
        self.uploadedCRC32 = uploadedCRC32
        self.retention = retention
        self.deviceExpiresAt = deviceExpiresAt
        self.deviceRetention = deviceRetention
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
