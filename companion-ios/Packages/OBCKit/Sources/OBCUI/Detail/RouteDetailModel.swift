import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the route-detail screen (B4) — **one profile layout, three
/// dressings** (the design's rule: don't fork the view):
///
///   • `.planned`  — E2, a saved route tapped from the Planned list
///   • `.tracked`  — E3, a device-recorded ride from the Tracked list
///   • `.imported` — E1, the landing for a just-parsed route file
///
/// Planned/tracked render their list summary immediately and fill in waypoints
/// + elevation when `routeDetail`/`rideDetail` land (a failed detail read
/// degrades quietly — the summary stats never depend on it). Imported computes
/// everything up front from the parsed geometry (`RouteStats`).
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

    /// Title — editable via `rename(to:)` on planned/tracked (H12).
    public private(set) var name: String
    /// Waypoints in ride order (W1); empty until the detail read lands.
    public private(set) var waypoints: [Waypoint] = []
    /// Elevation samples for the profile card; empty hides the card.
    public private(set) var elevationProfile: [Double] = []
    /// E2's MAX stat, when the source knows it.
    public private(set) var maxGradePercent: Double?

    // MARK: Fixed per-dressing facts

    public private(set) var preview: TrackPreview?
    /// The soft line under the title (E3's "Yesterday, 8:12 AM"; E1's file name).
    public let subtitle: String?
    public private(set) var distanceMeters: Double = 0
    private var climbMeters: Double = 0
    private var descentMeters: Double = 0
    private var estimatedDuration: TimeInterval?
    private var pointCount = 0
    /// Stats computed for an imported file (E1) — also what `makeSummary` saves.
    private var importedStats: RouteStats?
    /// The canonical geometry an upload encodes to OBCR. The imported dressing
    /// carries its own; a planned route's is threaded from the library (the device
    /// wire blob is re-encoded from it, per the B1S format rule). `nil` for a
    /// device-only planned route with no app-side geometry — it's already on the
    /// device, so the upload affordance reads "Uploaded" rather than re-pushing.
    @ObservationIgnored private let uploadGeometry: ImportedRoute?
    /// The device object id to replace on upload — non-nil when re-uploading a
    /// route already on the device (an edited Komoot re-import, or a planned
    /// re-push) so it updates in place instead of duplicating.
    @ObservationIgnored private let uploadTargetObjectID: UInt16?

    public var isRenamable: Bool {
        switch dressing {
        case .planned, .tracked: true
        case .imported: false  // the name saves with the route; no pencil on E1
        }
    }

    // MARK: Wiring

    private let transport: any DeviceTransport
    @ObservationIgnored private var started = false

    /// `preloadedDetail` short-circuits the transport fetch — the composition
    /// root passes it for routes saved from an import this session, whose
    /// waypoints/profile live app-side, not on the device.
    public init(
        transport: any DeviceTransport,
        dressing: Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        deviceObjectID: UInt16? = nil,
        importedRouteID: RouteID? = nil
    ) {
        self.transport = transport
        self.dressing = dressing
        self.uploadTargetObjectID = deviceObjectID
        self.importedID = importedRouteID ?? RouteID("imported-\(UUID().uuidString.lowercased())")
        switch dressing {
        case .imported(let route, _): uploadGeometry = route  // E1 carries its own geometry
        default: uploadGeometry = plannedGeometry
        }

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
                started = true  // nothing left to fetch
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

    /// Fetch the detail read for planned/tracked (call once, from `.task`).
    /// Imported already has everything; failures degrade quietly.
    public func start() {
        guard !started else { return }
        started = true
        switch dressing {
        case .planned(let route):
            Task { [transport] in
                guard let detail = try? await transport.routeDetail(route.id) else { return }
                waypoints = detail.waypoints
                elevationProfile = detail.elevationProfile
                maxGradePercent = detail.maxGradePercent
            }
        case .tracked(let ride):
            Task { [transport] in
                guard let detail = try? await transport.rideDetail(ride.id) else { return }
                elevationProfile = detail.elevationProfile
            }
        case .imported:
            break
        }
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

    /// The E1 banner line ("Imported from Komoot"); `nil` on E2/E3.
    public var importedFromLine: String? {
        guard case .imported(let route, let fileName) = dressing else { return nil }
        let creator = route.creator?.lowercased() ?? ""
        if creator.contains("komoot") { return "Imported from Komoot" }
        if creator.contains("strava") { return "Imported from Strava" }
        if creator.contains("garmin") { return "Imported from Garmin" }
        let ext = (fileName as NSString).pathExtension.uppercased()
        return ext.isEmpty ? "Imported route file" : "Imported from \(ext) file"
    }

    // MARK: Stat strip (E1/E2/E3 columns, per the design)

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

    /// H12 — local rename; the caller propagates it to the list (and the
    /// device gets it on next upload). Empty/whitespace names are ignored.
    public func rename(to newName: String) -> Bool {
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        name = trimmed
        return true
    }

    /// The id an E1 save/upload lands under — stable per landing, so the
    /// uploaded blob and the saved library entry are the same route. A re-import
    /// **replacing** an existing route reuses that route's id (passed in) so the
    /// save overwrites it rather than adding a duplicate.
    @ObservationIgnored private let importedID: RouteID

    /// The `RouteSummary` an E1 save/upload lands in the library — the parsed
    /// geometry's stats under a fresh (per-landing) id.
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

    /// The `RouteBlob` the upload sheet (B5) sends — the current name + waypoints
    /// over the **real OBCR v2 payload** the device stores verbatim and rides
    /// (`RouteObjectCodec`, spec §7.1). The geometry is the imported route's (E1)
    /// or the library's for a planned route; a device-only planned route with no
    /// app-side geometry sends nothing (it's already on the device).
    public func makeUploadBlob() -> RouteBlob {
        let summary: RouteSummary
        switch dressing {
        case .planned(var route):
            route.name = name  // a rename rides along (H12)
            summary = route
        case .imported, .tracked:  // tracked never uploads (E3 has no action)
            summary = makeSummary()
        }
        let payload = uploadGeometry.map {
            RouteObjectCodec.encode(points: $0.points, waypoints: waypoints, name: name)
        } ?? Data()
        return RouteBlob(
            summary: summary, waypoints: waypoints, payload: payload,
            targetObjectID: uploadTargetObjectID
        )
    }

    /// The full `RouteDetail` an E1 save keeps app-side — reopening the saved
    /// route must not lose the parsed waypoints/profile (the device never had
    /// them; the mock's `routeDetail` can't answer for a phone-only id).
    public func makeDetail() -> RouteDetail {
        RouteDetail(
            summary: makeSummary(),
            waypoints: waypoints,
            elevationProfile: elevationProfile,
            maxGradePercent: maxGradePercent
        )
    }
}
