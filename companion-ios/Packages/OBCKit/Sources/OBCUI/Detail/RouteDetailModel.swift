import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the route-detail screen: one profile layout, three dressings, because the view is
/// never forked.
///
/// A planned route is library-first: its waypoints and profile come in as `preloadedDetail`,
/// derived from the saved record's own geometry, so the screen never asks the device for a route
/// the phone already holds. Tracked renders its summary at once and fills the profile when the
/// ride detail lands; a failed read degrades quietly. Imported computes everything up front.
@MainActor @Observable
public final class RouteDetailModel {
    /// Which of the three dressings this instance wears.
    public enum Dressing {
        case planned(RouteSummary)
        case tracked(RideSummary)
        case imported(ImportedRoute, fileName: String)
    }

    public let dressing: Dressing

    // MARK: Observable state

    /// Title, editable through `rename(to:)` on every dressing.
    public private(set) var name: String
    /// Waypoints in ride order; empty until the detail read lands.
    public private(set) var waypoints: [Waypoint] = []
    /// Elevation samples for the profile card; empty hides the card.
    public private(set) var elevationProfile: [Double] = []
    public private(set) var maxGradePercent: Double?
    /// The live link state. Upload is link-bound, so the button dims with it. Starts optimistic;
    /// the stream's replayed value corrects it before the first frame on every transport.
    public private(set) var connection: ConnectionState = .connected

    public var canUpload: Bool { connection == .connected }

    // MARK: Fixed per-dressing facts

    public private(set) var preview: TrackPreview?
    /// The track the interactive map draws: full resolution when it is available, else the
    /// preview's own downsampled coordinates. Never empty when `preview` has geometry.
    public var mapCoordinates: [Coordinate] {
        !fullTrackCoordinates.isEmpty ? fullTrackCoordinates : (preview?.coordinates ?? [])
    }
    @ObservationIgnored private let fullTrackCoordinates: [Coordinate]
    /// The soft line under the title: a ride's date, or an imported file's name.
    public let subtitle: String?
    public private(set) var distanceMeters: Double = 0
    private var climbMeters: Double = 0
    private var descentMeters: Double = 0
    private var estimatedDuration: TimeInterval?
    private var pointCount = 0
    /// Stats computed for an imported file, which is also what `makeSummary` saves.
    private var importedStats: RouteStats?
    /// The canonical geometry an upload encodes. The imported dressing carries its own; a planned
    /// route's is threaded from the library, and planned routes are library-first, so it is always
    /// present where Upload shows. A defensive nil yields an empty payload the transports reject.
    @ObservationIgnored private let uploadGeometry: ImportedRoute?
    /// The device object id to replace on upload: non-nil when re-uploading a route the device
    /// already holds, so it updates in place instead of duplicating. Mutable, because the moment
    /// an upload commits `recordUploaded` pins the assigned id here, and pressing Upload again on
    /// the same screen replaces that object.
    private var uploadTargetObjectID: DeviceObjectID?
    /// The CRC the device is proven to currently hold for this route, threaded from the main
    /// model's identity-verified reconcile, or set by `recordUploaded` when an upload verified it.
    /// Nil is unproven, so the button reads Upload and never a checkmark on presence alone.
    private var provenCommittedCRC: UInt32?
    /// The current payload's CRC, encoded lazily and cached. A rename invalidates it, because the
    /// name is part of the payload.
    @ObservationIgnored private var cachedPayloadCRC: UInt32?

    /// The device-copy state behind the Upload, Update and up-to-date button.
    public var deviceCopyState: OnDeviceState {
        OnDeviceState.determine(
            provenCommittedCRC: provenCommittedCRC,
            currentCRC: { currentPayloadCRC() }
        )
    }

    /// An upload committed under `objectID`: pin the id and the verified fingerprint, so the
    /// button flips to up to date and any further upload replaces in place.
    public func recordUploaded(objectID: DeviceObjectID, crc32: UInt32) {
        uploadTargetObjectID = objectID
        provenCommittedCRC = crc32
    }

    private func currentPayloadCRC() -> UInt32 {
        if let cached = cachedPayloadCRC { return cached }
        let crc = CRC32.checksum(uploadPayload())
        cachedPayloadCRC = crc
        return crc
    }

    /// Every dressing renames. On the import landing the pencil fixes the name before save or
    /// upload, so an import does not have to round-trip through the Planned list.
    public var isRenamable: Bool { true }

    @ObservationIgnored private let now: () -> Date
    // MARK: Wiring

    private let transport: any DeviceLink & DeviceObjects
    @ObservationIgnored private var started = false
    @ObservationIgnored private var connectionWatch: Task<Void, Never>?

    /// `preloadedDetail` short-circuits the transport fetch: the composition root passes it for
    /// routes saved from an import, whose waypoints and profile live app-side, not on the device.
    public init(
        transport: any DeviceLink & DeviceObjects,
        dressing: Dressing,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        deviceObjectID: DeviceObjectID? = nil,
        provenCommittedCRC: UInt32? = nil,
        importedRouteID: RouteID? = nil,
        now: @escaping () -> Date = Date.init,
        // The tracked dressing's full tracklog, threaded from the library's synced ride points. A
        // ride carries no `ImportedRoute`, so it cannot ride along on `uploadGeometry`.
        rideGeometry: [Coordinate]? = nil
    ) {
        self.transport = transport
        self.dressing = dressing
        self.uploadTargetObjectID = deviceObjectID
        self.provenCommittedCRC = provenCommittedCRC
        self.now = now
        self.importedID = importedRouteID ?? RouteID("imported-\(UUID().uuidString.lowercased())")
        switch dressing {
        case .imported(let route, _): uploadGeometry = route
        default: uploadGeometry = plannedGeometry
        }
        // The interactive map draws this, never the downsampled `preview`. Full resolution is
        // already in memory for imported and planned routes; `rideGeometry` threads it in for
        // tracked. It falls back to the preview's coordinates when neither is available, which is
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

    /// Fetch the tracked dressing's detail read; failures degrade quietly. Planned and imported
    /// already have everything.
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

    /// The hero's corner tag, and whether it reads in the tracked accent colour.
    public var tag: (text: String, isAccent: Bool) {
        switch dressing {
        case .planned: ("Planned", false)
        case .tracked(let ride): ("Tracked · \(OBCFormat.rideDay(ride.date))", true)
        case .imported: ("New · unsaved", false)
        }
    }

    /// The import banner line; nil on the other dressings.
    public var importedFromLine: String? {
        guard case .imported(let route, let fileName) = dressing else { return nil }
        let creator = route.creator?.lowercased() ?? ""
        if creator.contains("komoot") { return "Imported from Komoot" }
        if creator.contains("strava") { return "Imported from Strava" }
        if creator.contains("garmin") { return "Imported from Garmin" }
        let ext = (fileName as NSString).pathExtension.uppercased()
        return ext.isEmpty ? "Imported route file" : "Imported from \(ext) file"
    }

    // MARK: Stat strip

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

    // MARK: Ride sensor summary (tracked only)

    /// One plain label and value row of the per-ride sensor summary.
    public struct SensorRow: Identifiable, Equatable, Sendable {
        public let label: String
        public let value: String
        public var id: String { label }
    }

    /// The tracked ride's sensor rows, in the design's fixed order: one row per value the ride
    /// actually carries, and nothing at all when it carries none. No dead rows for an absent value.
    public var sensorRows: [SensorRow] {
        guard case .tracked(let ride) = dressing else { return [] }
        var rows: [SensorRow] = []
        if let v = ride.avgHeartRate { rows.append(SensorRow(label: "Avg heart rate", value: "\(v) bpm")) }
        if let v = ride.maxHeartRate { rows.append(SensorRow(label: "Max heart rate", value: "\(v) bpm")) }
        if let v = ride.avgPower { rows.append(SensorRow(label: "Avg power", value: "\(v) W")) }
        if let v = ride.maxPower { rows.append(SensorRow(label: "Max power", value: "\(v) W")) }
        if let v = ride.avgCadence { rows.append(SensorRow(label: "Avg cadence", value: "\(v) rpm")) }
        return rows
    }

    // MARK: Actions

    /// A local rename; the caller propagates it to the list, and the device gets it on the next
    /// upload. Empty or whitespace names are ignored.
    public func rename(to newName: String) -> Bool {
        let trimmed = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        name = trimmed
        // The name rides in the payload, so a rename out-dates the device copy until the next one.
        cachedPayloadCRC = nil
        return true
    }

    /// The id an import's save or upload lands under, stable per landing, so the uploaded blob
    /// and the saved library entry are the same route. A re-import that replaces an existing route
    /// reuses that route's id, so the save overwrites instead of adding a duplicate.
    @ObservationIgnored private let importedID: RouteID

    /// The summary an import's save or upload lands in the library: the parsed geometry's stats.
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

    /// The blob the upload sheet sends: the current name and waypoints over the real OBCR payload
    /// the device stores verbatim and rides. The geometry is the imported route's, or the library
    /// record's for a planned route.
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

    /// The payload an upload sends, which is also what `deviceCopyState` fingerprints, so "up to
    /// date" always means byte-identical to this.
    private func uploadPayload() -> Data {
        uploadGeometry.map {
            RouteObjectCodec.encode(points: $0.points, waypoints: waypoints, name: name)
        } ?? Data()
    }

    /// The full detail an import's save keeps app-side: reopening the saved route must not lose
    /// the parsed waypoints and profile, because the device never had them.
    public func makeDetail() -> RouteDetail {
        RouteDetail(
            summary: makeSummary(),
            waypoints: waypoints,
            elevationProfile: elevationProfile,
            maxGradePercent: maxGradePercent
        )
    }
}
