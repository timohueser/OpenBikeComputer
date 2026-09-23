import Foundation
import Observation
import OBCDomain
import OBCTransport

/// State for the route-detail screen: one profile layout, four dressings, because the view is
/// never forked.
///
/// A planned route is library-first: its waypoints and profile come in as `preloadedDetail`,
/// derived from the saved record's own geometry, so the screen never asks the device for a route
/// the phone already holds. Tracked computes its profile and highlights from the synced ride
/// points in `start()`, which runs once on the live model. Imported computes everything up front.
@MainActor @Observable
public final class RouteDetailModel {
    /// Which of the four dressings this instance wears.
    public enum Dressing {
        case planned(RouteSummary)
        /// One day of a trip, read-only: the trip page owns its name, bike type and upload.
        case tripDay(RouteSummary)
        case tracked(RideSummary)
        case imported(ImportedRoute, fileName: String, source: ImportSource = .file)
    }

    public let dressing: Dressing

    // MARK: Observable state

    /// Title, editable through `rename(to:)` on every dressing.
    public private(set) var name: String
    /// Waypoints in ride order; empty until the detail read lands.
    public private(set) var waypoints: [Waypoint] = []
    /// Elevation samples for the profile card; empty hides the card.
    public private(set) var elevationProfile: [Double] = []
    /// A tracked ride's highlights, the most notable first; empty elsewhere.
    public private(set) var highlights: [String] = []
    public private(set) var maxGradePercent: Double?
    /// The type the estimate uses and an upload writes. Changed through `setBikeType(_:)`.
    public private(set) var bikeType: BikeType
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
    @ObservationIgnored private let ridePoints: [RidePoint]
    @ObservationIgnored private let rides: [RideSummary]
    /// The soft line under the title: a ride's date, or an imported file's name.
    public let subtitle: String?
    public private(set) var distanceMeters: Double = 0
    private var climbMeters: Double = 0
    private var descentMeters: Double = 0
    private var pointCount = 0
    /// The `OBCR_Spec.md` §1.2 estimate. Distance and climb hold the upload's header figures, so it
    /// is the estimate the device shows for this route and type.
    private var estimatedDuration: TimeInterval? {
        if case .tracked = dressing { return nil }
        return bikeType.estimatedDuration(distanceMeters: distanceMeters, ascentMeters: climbMeters)
    }
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

    /// The type rides in the payload, so a change out-dates the device copy until the next upload.
    public func setBikeType(_ type: BikeType) {
        bikeType = type
        cachedPayloadCRC = nil
    }

    private func currentPayloadCRC() -> UInt32 {
        if let cached = cachedPayloadCRC { return cached }
        let crc = CRC32.checksum(uploadPayload())
        cachedPayloadCRC = crc
        return crc
    }

    /// On the import landing the pencil fixes the name before save or upload, so an import does
    /// not have to round-trip through the Planned list. A trip day is renamed on the trip page.
    public var isRenamable: Bool {
        if case .tripDay = dressing { return false }
        return true
    }

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
        bikeType: BikeType = .road,
        preloadedDetail: RouteDetail? = nil,
        plannedGeometry: ImportedRoute? = nil,
        deviceObjectID: DeviceObjectID? = nil,
        provenCommittedCRC: UInt32? = nil,
        importedRouteID: RouteID? = nil,
        now: @escaping () -> Date = Date.init,
        // The tracked dressing's full tracklog, from the library's synced ride. A ride carries no
        // `ImportedRoute`, so it cannot ride along on `uploadGeometry`.
        ridePoints: [RidePoint] = [],
        // Every synced ride summary: the tracked dressing finds its trip's biggest day in it.
        rides: [RideSummary] = []
    ) {
        self.transport = transport
        self.dressing = dressing
        self.bikeType = bikeType
        self.uploadTargetObjectID = deviceObjectID
        self.provenCommittedCRC = provenCommittedCRC
        self.now = now
        self.importedID = importedRouteID ?? RouteID("imported-\(UUID().uuidString.lowercased())")
        switch dressing {
        case .imported(let route, _, _): uploadGeometry = route
        default: uploadGeometry = plannedGeometry
        }
        // The interactive map draws this, never the downsampled `preview`. Full resolution is
        // already in memory for imported and planned routes; `ridePoints` threads it in for
        // tracked. It falls back to the preview's coordinates when neither is available, which is
        // a coarser map, not a missing one.
        fullTrackCoordinates = uploadGeometry?.points.map(\.coordinate) ?? ridePoints.map(\.coordinate)
        self.ridePoints = ridePoints
        self.rides = rides

        switch dressing {
        case .planned(let route), .tripDay(let route):
            name = route.name
            subtitle = nil
            preview = route.trackPreview
            distanceMeters = route.distanceMeters
            climbMeters = route.elevationGainMeters
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

        case .imported(let route, let fileName, let source):
            let stats = RouteStats.compute(from: route.points)
            name = route.name ?? fileName
            // A ride's file name is ours, not a file the rider picked.
            subtitle = source == .file ? fileName : nil
            preview = TrackPreview.normalizing(route.points.map(\.coordinate))
            // The header figures, which are what the device shows for this route.
            if let totals = RouteObjectCodec.totals(points: route.points) {
                distanceMeters = Double(totals.distanceMeters)
                climbMeters = Double(totals.ascentMeters)
                descentMeters = Double(totals.descentMeters)
            } else {
                distanceMeters = stats.distanceMeters
                climbMeters = stats.elevationGainMeters
                descentMeters = stats.elevationLossMeters
            }
            pointCount = route.points.count
            waypoints = route.waypoints
            elevationProfile = stats.elevationProfile
            maxGradePercent = stats.maxGradePercent
        }
    }

    /// Watch the link, and fill a tracked ride's profile and highlights. A host may build throwaway
    /// models on every render, so this whole-track work waits for the live one.
    public func start() {
        guard !started else { return }
        started = true
        if case .tracked(let summary) = dressing {
            let ride = Ride(summary: summary, points: ridePoints)
            elevationProfile = MeasuredLine.elevationProfile(ridePoints: ridePoints)
            highlights = RideHighlights.compute(ride, library: rides).map { OBCFormat.highlight($0) }
        }
        connectionWatch = Task { [weak self, transport] in
            for await state in transport.state {
                guard let self else { return }
                connection = state
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
        case .planned, .tripDay: ("Planned", false)
        case .tracked(let ride): ("Tracked · \(OBCFormat.rideDay(ride.date))", true)
        case .imported: ("New · unsaved", false)
        }
    }

    /// The landing's navigation title.
    public var landingTitle: String {
        guard case .imported(_, _, .ride) = dressing else { return "Imported route" }
        return "New route"
    }

    /// The import banner line; nil on the other dressings.
    public var importedFromLine: String? {
        guard case .imported(let route, let fileName, let source) = dressing else { return nil }
        if case .ride(let date) = source { return "From ride · \(OBCFormat.rideDay(date))" }
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
        case .planned, .tripDay:
            [
                OBCStat(value: OBCFormat.distanceValue(meters: distanceMeters), unit: "km", key: "Distance"),
                OBCStat(value: OBCFormat.climbValue(meters: climbMeters), unit: "m", key: "Climb"),
                estimateStat,
                maxGradePercent.map {
                    OBCStat(value: "\(Int($0.rounded()))", unit: "%", key: "Max")
                } ?? OBCStat(value: "—", key: "Max"),
            ]
        case .tracked:
            []  // a ride has the one stats line instead
        case .imported:
            [
                OBCStat(value: OBCFormat.distanceValue(meters: distanceMeters), unit: "km", key: "Distance"),
                OBCStat(value: OBCFormat.climbValue(meters: climbMeters), unit: "m", key: "Climb"),
                OBCStat(value: OBCFormat.climbValue(meters: descentMeters), unit: "m", key: "Descent"),
                estimateStat,
            ]
        }
    }

    /// A tracked ride's one stats line under its title; nil on the other dressings.
    public var statsLine: String? {
        guard case .tracked(let ride) = dressing else { return nil }
        return OBCFormat.rideStatsLine(ride)
    }

    /// Whole minutes, floored, as the device route overview shows the same estimate.
    private var estimateStat: OBCStat {
        OBCStat(value: estimatedDuration.map { OBCFormat.movingTime(($0 / 60).rounded(.down) * 60) } ?? "—", key: "Est. time")
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

    /// The summary an import's save or upload lands in the library: the stat strip's figures.
    public func makeSummary() -> RouteSummary {
        var source = RouteSource.gpx
        if case .imported(_, let fileName, _) = dressing,
            (fileName as NSString).pathExtension.lowercased() == "tcx" {
            source = .tcx
        }
        return RouteSummary(
            id: importedID,
            name: name,
            distanceMeters: distanceMeters,
            elevationGainMeters: climbMeters,
            estimatedDuration: estimatedDuration,
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
            route.estimatedDuration = estimatedDuration
            summary = route
        case .imported, .tracked, .tripDay:  // tracked and trip days never upload
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
            RouteObjectCodec.encode(points: $0.points, waypoints: waypoints, name: name, bikeType: bikeType)
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

/// Where a route on the import landing comes from, which picks the landing's copy.
public enum ImportSource: Equatable, Sendable {
    /// A route file the rider picked or shared.
    case file
    /// A tracked ride saved as a route, recorded on this date.
    case ride(Date)
}
