import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the route-detail screen — **one profile layout, three dressings**
/// (don't fork the view):
///
///   • `.planned`  — a saved route tapped from the Planned list
///   • `.tracked`  — a device-recorded ride from the Tracked list
///   • `.imported` — the landing for a just-parsed route file
///
/// Planned routes are **library-first**: the waypoints + profile come in as
/// `preloadedDetail`, derived from the saved record's own geometry — planned
/// never asks the device for a route the phone already holds. Tracked renders
/// its summary immediately and fills the profile when `rideDetail` lands (a
/// failed read degrades quietly). Imported computes everything up front from
/// the parsed geometry (`RouteStats`).
@MainActor @Observable
public final class RouteDetailModel {
    /// Which of the three design dressings this instance wears.
    public enum Dressing {
        case planned(RouteSummary)
        case tracked(RideSummary)
        case imported(ImportedRoute, fileName: String)
    }

    public let dressing: Dressing

    // MARK: Observable state

    /// Title — editable via `rename(to:)` on planned/tracked.
    public private(set) var name: String
    /// Waypoints in ride order; empty until the detail read lands.
    public private(set) var waypoints: [Waypoint] = []
    /// Elevation samples for the profile card; empty hides the card.
    public private(set) var elevationProfile: [Double] = []
    /// The MAX stat, when the source knows it.
    public private(set) var maxGradePercent: Double?
    /// The live link state — Upload is link-bound, so the button dims with it.
    /// Starts optimistic; the stream's replayed value corrects it before the
    /// first frame on every transport.
    public private(set) var connection: ConnectionState = .connected

    /// Whether Upload can act right now.
    public var canUpload: Bool { connection == .connected }

    // MARK: Fixed per-dressing facts

    public private(set) var preview: TrackPreview?
    /// The track the interactive map draws — full resolution when it's
    /// available (imported/planned always; tracked when `rideGeometry` was
    /// threaded in), else the preview's own downsampled coordinates. Never
    /// empty when `preview` has geometry, so `canExpandMap`-style checks can
    /// key on it directly.
    public var mapCoordinates: [Coordinate] {
        !fullTrackCoordinates.isEmpty ? fullTrackCoordinates : (preview?.coordinates ?? [])
    }
    @ObservationIgnored private let fullTrackCoordinates: [Coordinate]
    /// The soft line under the title (tracked's "Yesterday, 8:12 AM"; imported's
    /// file name).
    public let subtitle: String?
    public private(set) var distanceMeters: Double = 0
    private var climbMeters: Double = 0
    private var descentMeters: Double = 0
    private var estimatedDuration: TimeInterval?
    private var pointCount = 0
    /// Stats computed for an imported file — also what `makeSummary` saves.
    private var importedStats: RouteStats?
    /// The canonical geometry an upload encodes to OBCR. The imported dressing
    /// carries its own; a planned route's is threaded from the library (the
    /// device wire blob is re-encoded from it). Planned routes are
    /// library-first, so this is always present where Upload shows; a
    /// defensive `nil` yields an empty payload the transports reject loudly.
    @ObservationIgnored private let uploadGeometry: ImportedRoute?
    /// The device object id to replace on upload — non-nil when re-uploading a
    /// route already on the device (an edited Komoot re-import, or a planned
    /// re-push) so it updates in place instead of duplicating. **Mutable**: the
    /// moment an upload commits, `recordUploaded` pins the assigned id here, so
    /// pressing Upload again on the same screen replaces that object instead of
    /// creating another copy.
    private var uploadTargetObjectID: UInt16?
    /// The committed payload's CRC-32 (the `OnDeviceState` fingerprint) —
    /// threaded from the library record, refreshed by `recordUploaded`.
    private var uploadedCRC32: UInt32?
    /// The current payload's CRC, encoded lazily and cached — a rename
    /// invalidates it (the name is part of the payload).
    @ObservationIgnored private var cachedPayloadCRC: UInt32?

    /// The device-copy state behind the Upload ↔ Update ↔ up-to-date button.
    public var deviceCopyState: OnDeviceState {
        OnDeviceState.determine(
            deviceObjectID: uploadTargetObjectID,
            uploadedCRC32: uploadedCRC32,
            currentCRC: { currentPayloadCRC() }
        )
    }

    /// An upload committed under `objectID`: pin the id + fingerprint so the
    /// button flips to up-to-date and any further upload replaces in place.
    public func recordUploaded(objectID: UInt16, crc32: UInt32) {
        uploadTargetObjectID = objectID
        uploadedCRC32 = crc32
    }

    private func currentPayloadCRC() -> UInt32 {
        if let cached = cachedPayloadCRC { return cached }
        let crc = CRC32.checksum(uploadPayload())
        cachedPayloadCRC = crc
        return crc
    }

    public var isRenamable: Bool {
        switch dressing {
        case .planned, .tracked: true
        case .imported: false  // the name saves with the route; no pencil on import
        }
    }

    // MARK: Wiring

    private let transport: any DeviceTransport
    @ObservationIgnored private var started = false
    @ObservationIgnored private var connectionWatch: Task<Void, Never>?

    /// `preloadedDetail` short-circuits the transport fetch — the composition
    /// root passes it for routes saved from an import this session, whose
    /// waypoints/profile live app-side, not on the device.
    public init(
        transport: any DeviceTransport,
        dressing: Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        deviceObjectID: UInt16? = nil,
        uploadedCRC32: UInt32? = nil,
        importedRouteID: RouteID? = nil,
        // The tracked dressing's full tracklog, threaded from the library's
        // synced `Ride.points` — a ride carries no ImportedRoute, so it can't
        // ride along on `uploadGeometry` the way planned/imported do.
        rideGeometry: [Coordinate]? = nil
    ) {
        self.transport = transport
        self.dressing = dressing
        self.uploadTargetObjectID = deviceObjectID
        self.uploadedCRC32 = uploadedCRC32
        self.importedID = importedRouteID ?? RouteID("imported-\(UUID().uuidString.lowercased())")
        switch dressing {
        case .imported(let route, _): uploadGeometry = route  // imported carries its own geometry
        default: uploadGeometry = plannedGeometry
        }
        // The interactive map draws this, never the downsampled `preview` —
        // full resolution is already in memory for imported/planned (it's the
        // same geometry `uploadGeometry` carries); `rideGeometry` threads it in
        // for tracked. Falls back to the preview's coordinates when neither is
        // available (a ride synced before this geometry was threaded through) —
        // a coarser map, not a missing one.
        fullTrackCoordinates = uploadGeometry?.points.map(\.coordinate) ?? rideGeometry ?? []

        switch dressing {
        case .planned(let route):
            name = route.name
            subtitle = nil
            preview = route.trackPreview
            distanceMeters = route.distanceMeters
            climbMeters = route.elevationGainMeters
            estimatedDuration = route.estimatedDuration
            pointCount = route.pointCount
            if let detail = preloadedDetail {
                waypoints = detail.waypoints
                elevationProfile = detail.elevationProfile
                maxGradePercent = detail.maxGradePercent
            }

        case .tracked(let ride):
            name = ride.name
            subtitle = OBCFormat.rideDateLine(ride.date)
            preview = ride.trackPreview
            distanceMeters = ride.distanceMeters
            climbMeters = ride.climbMeters

        case .imported(let route, let fileName):
            let stats = RouteStats.compute(from: route.points)
            importedStats = stats
            name = route.name ?? fileName
            subtitle = fileName
            preview = TrackPreview.normalizing(route.points.map(\.coordinate))
            distanceMeters = stats.distanceMeters
            climbMeters = stats.elevationGainMeters
            descentMeters = stats.elevationLossMeters
            estimatedDuration = stats.estimatedDuration
            pointCount = route.points.count
            waypoints = route.waypoints
            elevationProfile = stats.elevationProfile
            maxGradePercent = stats.maxGradePercent
        }
    }

    /// Fetch the tracked dressing's detail read (call once, from `.task`);
    /// failures degrade quietly. Planned and imported already have everything —
    /// planned from its library record (`preloadedDetail`), imported from the
    /// parsed geometry.
    public func start() {
        guard !started else { return }
        started = true
        connectionWatch = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
            }
        }
        switch dressing {
        case .planned, .imported:
            break
        case .tracked(let ride):
            Task { [transport] in
                guard let detail = try? await transport.rideDetail(ride.id) else { return }
                elevationProfile = detail.elevationProfile
            }
        }
    }

    deinit {
        connectionWatch?.cancel()
    }

    // MARK: Header dressing

    /// The hero's corner tag + whether it reads in forest (the tracked accent).
    public var tag: (text: String, isAccent: Bool) {
        switch dressing {
        case .planned: ("Planned", false)
        case .tracked(let ride): ("Tracked · \(OBCFormat.rideDay(ride.date))", true)
        case .imported: ("New · unsaved", false)
        }
    }

    /// The import banner line ("Imported from Komoot"); `nil` on planned/tracked.
    public var importedFromLine: String? {
        guard case .imported(let route, let fileName) = dressing else { return nil }
        let creator = route.creator?.lowercased() ?? ""
        if creator.contains("komoot") { return "Imported from Komoot" }
        if creator.contains("strava") { return "Imported from Strava" }
        if creator.contains("garmin") { return "Imported from Garmin" }
        let ext = (fileName as NSString).pathExtension.uppercased()
        return ext.isEmpty ? "Imported route file" : "Imported from \(ext) file"
    }

    // MARK: Stat strip (per-dressing columns)

    public var stats: [OBCStat] {
        switch dressing {
        case .planned:
            [
                OBCStat(value: OBCFormat.distanceValue(meters: distanceMeters), unit: "km", key: "Distance"),
                OBCStat(value: OBCFormat.climbValue(meters: climbMeters), unit: "m", key: "Climb"),
                OBCStat(value: estimatedDuration.map { OBCFormat.movingTime($0) } ?? "—", key: "Est. time"),
                maxGradePercent.map {
                    OBCStat(value: "\(Int($0.rounded()))", unit: "%", key: "Max")
                } ?? OBCStat(value: "—", key: "Max"),
            ]
        case .tracked(let ride):
            [
                OBCStat(value: OBCFormat.distanceValue(meters: ride.distanceMeters), unit: "km", key: "Distance"),
                OBCStat(value: OBCFormat.movingTime(ride.movingTime), key: "Moving"),
                OBCStat(value: OBCFormat.speedValue(mps: ride.averageSpeedMps), unit: "kph", key: "Avg"),
                OBCStat(value: OBCFormat.climbValue(meters: ride.climbMeters), unit: "m", key: "Climb"),
            ]
        case .imported:
            [
                OBCStat(value: OBCFormat.distanceValue(meters: distanceMeters), unit: "km", key: "Distance"),
                OBCStat(value: OBCFormat.climbValue(meters: climbMeters), unit: "m", key: "Climb"),
                OBCStat(value: OBCFormat.climbValue(meters: descentMeters), unit: "m", key: "Descent"),
                OBCStat(value: estimatedDuration.map { OBCFormat.movingTime($0) } ?? "—", key: "Est. time"),
            ]
        }
    }

    // MARK: Actions

    /// Local rename; the caller propagates it to the list (and the device
    /// gets it on next upload). Empty/whitespace names are ignored.
    public func rename(to newName: String) -> Bool {
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        name = trimmed
        // The name rides in the payload: a rename out-dates the device copy
        // until the next upload pushes it.
        cachedPayloadCRC = nil
        return true
    }

    /// The id an import save/upload lands under — stable per landing, so the
    /// uploaded blob and the saved library entry are the same route. A re-import
    /// **replacing** an existing route reuses that route's id (passed in) so the
    /// save overwrites it rather than adding a duplicate.
    @ObservationIgnored private let importedID: RouteID

    /// The `RouteSummary` an import save/upload lands in the library — the
    /// parsed geometry's stats under a fresh (per-landing) id.
    public func makeSummary() -> RouteSummary {
        let stats = importedStats ?? RouteStats(distanceMeters: distanceMeters, elevationGainMeters: climbMeters)
        var source = RouteSource.gpx
        if case .imported(_, let fileName) = dressing,
            (fileName as NSString).pathExtension.lowercased() == "tcx" {
            source = .tcx
        }
        return RouteSummary(
            id: importedID,
            name: name,
            distanceMeters: stats.distanceMeters,
            elevationGainMeters: stats.elevationGainMeters,
            estimatedDuration: stats.estimatedDuration,
            pointCount: pointCount,
            source: source,
            trackPreview: preview
        )
    }

    /// The `RouteBlob` the upload sheet sends — the current name + waypoints
    /// over the **real OBCR v2 payload** the device stores verbatim and rides
    /// (`RouteObjectCodec`, spec §7.1). The geometry is the imported route's or
    /// the library record's for a planned route (every planned row is a
    /// library save, so it's always there).
    public func makeUploadBlob() -> RouteBlob {
        let summary: RouteSummary
        switch dressing {
        case .planned(var route):
            route.name = name  // a rename rides along
            summary = route
        case .imported, .tracked:  // tracked never uploads
            summary = makeSummary()
        }
        return RouteBlob(
            summary: summary, waypoints: waypoints, payload: uploadPayload(),
            targetObjectID: uploadTargetObjectID
        )
    }

    /// The OBCR payload an upload sends — also what `deviceCopyState`
    /// fingerprints, so "up to date" always means byte-identical to this.
    private func uploadPayload() -> Data {
        uploadGeometry.map {
            RouteObjectCodec.encode(points: $0.points, waypoints: waypoints, name: name)
        } ?? Data()
    }

    /// The full `RouteDetail` an import save keeps app-side — reopening the
    /// saved route must not lose the parsed waypoints/profile (the device
    /// never had them; the mock's `routeDetail` can't answer for a phone-only id).
    public func makeDetail() -> RouteDetail {
        RouteDetail(
            summary: makeSummary(),
            waypoints: waypoints,
            elevationProfile: elevationProfile,
            maxGradePercent: maxGradePercent
        )
    }
}
