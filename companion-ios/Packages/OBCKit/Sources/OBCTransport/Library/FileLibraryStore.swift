import Foundation
import OBCDomain

/// Canonical JSON library in app-private storage. Each ride has an atomic summary
/// manifest that names an immutable points file. Lists never decode point files.
/// Full ride writes use file and directory persistence barriers; other library
/// features retain their own write semantics.
public struct FileLibraryStore: LibraryStore, Sendable {
    private let directory: URL
    private var durabilityRoot: URL
    private var archiveCheckpoint: @Sendable (ArchiveCheckpoint) throws -> Void = { _ in }

    /// `directory` is created on first use. Its parent must be an existing,
    /// durable directory that the caller can open and sync.
    public init(directory: URL) {
        self.directory = directory.standardizedFileURL
        self.durabilityRoot = directory.standardizedFileURL.deletingLastPathComponent()
    }

    /// Fault seam for the archive transaction, before each named operation.
    enum ArchiveCheckpoint: CaseIterable { case pointsWrite, pointsSync, manifestWrite, manifestSync, publish, directorySync }

    init(directory: URL, archiveCheckpoint: @escaping @Sendable (ArchiveCheckpoint) throws -> Void) {
        self.init(directory: directory)
        self.archiveCheckpoint = archiveCheckpoint
    }

    /// The production location: Application Support, backed up, app-private.
    public static func standard() -> FileLibraryStore {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        var store = FileLibraryStore(directory: base.appendingPathComponent("OBCLibrary", isDirectory: true))
        // Library exists inside the app container; Application Support can be created below it.
        store.durabilityRoot = base.deletingLastPathComponent()
        return store
    }

    // MARK: Planned routes

    public func plannedRoutes() -> [PlannedRouteRecord] {
        contents(of: plannedDir)
            .compactMap { dir -> PlannedRouteRecord? in
                guard let file: PlannedRouteFile = read(dir.appendingPathComponent("route.json")),
                    file.version == Self.schemaVersion
                else { return nil }
                let source = dir.appendingPathComponent(Self.sourceName(for: file.sourceFileName))
                return file.record(sourceFileData: (try? Data(contentsOf: source)) ?? Data())
            }
            .sorted { $0.addedAt > $1.addedAt }
    }

    public func savePlannedRoute(_ record: PlannedRouteRecord) {
        let dir = plannedDir.appendingPathComponent(Self.fileSafe(record.id.rawValue), isDirectory: true)
        ensure(dir)
        write(PlannedRouteFile(record), to: dir.appendingPathComponent("route.json"))
        writeSourceSidecar(record.sourceFileData, named: record.sourceFileName, in: dir)
    }

    /// Persist the byte-exact original import file as the `source.<ext>` sidecar.
    /// A **replace-import** reuses the record's id, so the sidecar already exists
    /// and may carry both different bytes *and* a different extension (GPX→TCX) —
    /// rewrite when the content changed and sweep any stale-extension sidecar, so
    /// `plannedRoutes()` never reads the old file (or an empty `Data()`). A plain
    /// rename keeps the same bytes, so the multi-MB write is still skipped.
    private func writeSourceSidecar(_ data: Data, named fileName: String, in dir: URL) {
        let targetName = Self.sourceName(for: fileName)
        let target = dir.appendingPathComponent(targetName)
        // Drop any earlier sidecar under a different extension (a format change).
        for url in contents(of: dir)
        where url.lastPathComponent.hasPrefix("source.") && url.lastPathComponent != targetName {
            try? FileManager.default.removeItem(at: url)
        }
        // Only touch the file when it's missing or its bytes actually changed.
        if (try? Data(contentsOf: target)) != data {
            try? data.write(to: target, options: .atomic)
        }
    }

    public func deletePlannedRoute(_ id: RouteID) {
        try? FileManager.default.removeItem(
            at: plannedDir.appendingPathComponent(Self.fileSafe(id.rawValue), isDirectory: true))
        // Prune the route from any trip that held it (a trip left empty dissolves).
        for var trip in storedTripRecords() where trip.stageIDs.contains(id) {
            trip.stageIDs.removeAll { $0 == id }
            if trip.stageIDs.isEmpty {
                removeTripFile(trip.id)
            } else {
                writeTrip(trip)
            }
        }
    }

    // MARK: Trips

    public func trips() -> [TripRecord] {
        let alive = existingRouteIDs()
        return storedTripRecords()
            .compactMap { trip -> TripRecord? in
                var trip = trip
                // Drop dangling stage ids (a route record gone out from under the
                // trip); a trip with nothing resolvable left is dropped.
                trip.stageIDs = trip.stageIDs.filter(alive.contains)
                return trip.stageIDs.isEmpty ? nil : trip
            }
            .sorted { $0.addedAt > $1.addedAt }
    }

    public func saveTrip(_ record: TripRecord) {
        writeTrip(record)
        // Invariant: a RouteID lives in ≤ 1 trip — strip the saved trip's stages
        // from every other stored trip; one thereby emptied dissolves.
        let claimed = Set(record.stageIDs)
        for var other in storedTripRecords() where other.id != record.id {
            let kept = other.stageIDs.filter { !claimed.contains($0) }
            guard kept.count != other.stageIDs.count else { continue }
            if kept.isEmpty {
                removeTripFile(other.id)
            } else {
                other.stageIDs = kept
                writeTrip(other)
            }
        }
    }

    public func deleteTrip(_ id: TripID) {
        removeTripFile(id)
    }

    /// Every stored trip, unpruned (the raw on-disk view the invariant + prune
    /// logic operate on; `trips()` is the pruned public read).
    private func storedTripRecords() -> [TripRecord] {
        contents(of: tripsDir).compactMap { url -> TripRecord? in
            guard url.pathExtension == "json",
                let file: TripFile = read(url), file.version == Self.tripSchemaVersion
            else { return nil }
            return file.record
        }
    }

    /// The set of planned-route ids currently on disk — read from each
    /// `route.json` (no source-sidecar load), the alive-set the trip read prunes
    /// dangling stages against.
    private func existingRouteIDs() -> Set<RouteID> {
        Set(
            contents(of: plannedDir).compactMap { dir -> RouteID? in
                guard let file: PlannedRouteFile = read(dir.appendingPathComponent("route.json")),
                    file.version == Self.schemaVersion
                else { return nil }
                return RouteID(file.summary.id)
            })
    }

    private func writeTrip(_ record: TripRecord) {
        ensure(tripsDir)
        write(TripFile(record), to: tripFileURL(record.id))
    }

    private func removeTripFile(_ id: TripID) {
        try? FileManager.default.removeItem(at: tripFileURL(id))
    }

    private func tripFileURL(_ id: TripID) -> URL {
        tripsDir.appendingPathComponent("\(Self.fileSafe(id.rawValue)).json")
    }

    // MARK: Tracked rides

    public func rideSummaries() -> [RideSummary] {
        contents(of: ridesDir)
            .compactMap { url -> RideSummary? in
                guard url.hasDirectoryPath,
                    let file: RideSummaryFile = read(url.appendingPathComponent("summary.json")),
                    file.version == Self.rideSchemaVersion
                else { return nil }
                return file.summary.domain
            }
            .sorted { $0.date > $1.date }
    }

    public func ridePoints(_ id: RideID) -> [RidePoint]? {
        guard let manifest = rideManifest(id),
              let file: RidePointsFile = read(rideDir(id).appendingPathComponent(manifest.pointsFile)),
              file.version == Self.rideSchemaVersion else { return nil }
        return file.points.map(\.domain)
    }

    public func saveRide(_ ride: Ride) throws {
        try commitRide(ride, downloaded: false)
    }

    public func archiveRide(_ ride: Ride) throws -> RideArchiveReceipt? {
        if let source = ride.summary.source, !source.matches(ride.id) {
            throw RideArchiveError.invalidSource
        }
        try commitRide(ride, downloaded: true)
        return ride.summary.source.map { RideArchiveReceipt(source: $0) }
    }

    public func archivedRideReceipt(_ id: RideID) -> RideArchiveReceipt? {
        archivedRideSource(id).map { RideArchiveReceipt(source: $0) }
    }

    public func archivedRideSource(_ id: RideID) -> RideSource? {
        guard let manifest = rideManifest(id), manifest.downloaded,
              let source = manifest.summary.source, source.matches(id) else { return nil }
        do {
            let pointsURL = rideDir(id).appendingPathComponent(manifest.pointsFile)
            let bytes = try Data(contentsOf: pointsURL)
            guard bytes.count == manifest.pointsLength,
                  CRC32.checksum(bytes) == manifest.pointsCRC32 else { return nil }
            // A previous process can stop after rename but before the final barrier.
            // Stabilize that visible generation before it can suppress another download.
            try archiveCheckpoint(.pointsSync)
            try DurableArchiveIO.syncFile(pointsURL)
            try archiveCheckpoint(.manifestSync)
            try DurableArchiveIO.syncFile(rideDir(id).appendingPathComponent("summary.json"))
            try archiveCheckpoint(.directorySync)
            try DurableArchiveIO.syncAncestors(rideDir(id), through: durabilityRoot)
            try DurableArchiveIO.syncFile(rideDir(id).appendingPathComponent("summary.json"))
            return source
        } catch { return nil }
    }

    private func rideManifest(_ id: RideID) -> RideSummaryFile? {
        guard let file: RideSummaryFile = read(rideDir(id).appendingPathComponent("summary.json")),
              file.version == Self.rideSchemaVersion,
              file.summary.id == id.rawValue, file.validPointsName else { return nil }
        return file
    }

    private func commitRide(_ ride: Ride, downloaded: Bool) throws {
        let points = try encode(RidePointsFile(ride.points), formatting: [.sortedKeys])
        let pointsName = "points-\(UUID().uuidString).json"
        let manifest = RideSummaryFile(ride.summary, pointsFile: pointsName,
                                       pointsLength: points.count, pointsCRC32: CRC32.checksum(points),
                                       downloaded: downloaded)
        let summary = try encode(manifest)
        let dir = rideDir(ride.id)
        let summaryURL = dir.appendingPathComponent("summary.json")
        if FileManager.default.fileExists(atPath: summaryURL.path), rideManifest(ride.id) == nil {
            throw RideArchiveError.unreadableArchive
        }
        // Only our own unreferenced transaction files may occupy an unfinished archive.
        if rideManifest(ride.id) == nil,
           contents(of: dir).contains(where: { !RideSummaryFile.isTransactionFile($0.lastPathComponent) }) {
            throw RideArchiveError.unreadableArchive
        }
        try DurableArchiveIO.createDirectory(dir, beneath: durabilityRoot)
        let pointsURL = dir.appendingPathComponent(pointsName)
        let pending = dir.appendingPathComponent("manifest-\(UUID().uuidString).json")
        var published = false
        defer {
            if !published { try? FileManager.default.removeItem(at: pending) }
            if !published { try? FileManager.default.removeItem(at: pointsURL) }
        }
        try archiveCheckpoint(.pointsWrite)
        try points.write(to: pointsURL, options: .withoutOverwriting)
        try archiveCheckpoint(.pointsSync)
        try DurableArchiveIO.syncFile(pointsURL)
        try DurableArchiveIO.syncAncestors(dir, through: durabilityRoot)
        try archiveCheckpoint(.manifestWrite)
        try summary.write(to: pending, options: .withoutOverwriting)
        try archiveCheckpoint(.manifestSync)
        try DurableArchiveIO.syncFile(pending)
        try archiveCheckpoint(.publish)
        try DurableArchiveIO.rename(pending, to: summaryURL)
        published = true
        try archiveCheckpoint(.directorySync)
        try DurableArchiveIO.syncAncestors(dir, through: durabilityRoot)
        try DurableArchiveIO.syncFile(summaryURL)
        // A failed/uncertain commit keeps both generations for safe retry.
        for file in contents(of: dir)
        where RideSummaryFile.isTransactionFile(file.lastPathComponent)
            && file.lastPathComponent != pointsName {
            try? FileManager.default.removeItem(at: file)
        }
    }

    public func saveRideSummary(_ summary: RideSummary) {
        guard var manifest = rideManifest(summary.id) else { return }
        // A local rename cannot change the archived device identity.
        var updated = summary
        updated.source = manifest.summary.source
        manifest.summary = RideSummaryDTO(updated)
        write(manifest, to: rideDir(summary.id).appendingPathComponent("summary.json"))
    }

    public func deleteRide(_ id: RideID) {
        guard rideManifest(id) != nil else { return }
        try? FileManager.default.removeItem(at: rideDir(id))
    }

    public func syncedRideIDs() -> Set<RideID> {
        syncedRideHistory().union(downloadedRideIDs())
    }

    private func syncedRideHistory() -> Set<RideID> {
        guard let file: SyncedRidesFile = read(syncedURL), file.version == Self.schemaVersion
        else { return [] }
        return Set(file.ids.map(RideID.init))
    }

    private func downloadedRideIDs() -> Set<RideID> {
        Set(contents(of: ridesDir).compactMap { dir in
            guard let file: RideSummaryFile = read(dir.appendingPathComponent("summary.json")),
                  file.version == Self.rideSchemaVersion, file.downloaded else { return nil }
            return RideID(file.summary.id)
        })
    }

    public func markRideSynced(_ id: RideID) {
        var ids = syncedRideHistory()
        guard ids.insert(id).inserted else { return }
        ensure(directory)
        write(SyncedRidesFile(version: Self.schemaVersion, ids: ids.map(\.rawValue).sorted()), to: syncedURL)
    }

    public func deletedRideIDs() -> Set<RideID> {
        guard let file: SyncedRidesFile = read(deletedURL), file.version == Self.schemaVersion
        else { return [] }
        return Set(file.ids.map(RideID.init))
    }

    public func markRideDeleted(_ id: RideID) {
        var ids = deletedRideIDs()
        guard ids.insert(id).inserted else { return }
        ensure(directory)
        write(SyncedRidesFile(version: Self.schemaVersion, ids: ids.map(\.rawValue).sorted()), to: deletedURL)
    }

    public func trashedRideIDs() -> [RideID: Date] {
        guard let file: TrashedRidesFile = read(trashedURL), file.version == Self.schemaVersion
        else { return [:] }
        return Dictionary(
            file.entries.map { (RideID($0.id), $0.trashedAt) },
            uniquingKeysWith: { first, _ in first }
        )
    }

    public func markRideTrashed(_ id: RideID, at date: Date) {
        var ids = trashedRideIDs()
        ids[id] = date
        writeTrashed(ids)
    }

    public func unmarkRideTrashed(_ id: RideID) {
        var ids = trashedRideIDs()
        guard ids.removeValue(forKey: id) != nil else { return }
        writeTrashed(ids)
    }

    private func writeTrashed(_ ids: [RideID: Date]) {
        ensure(directory)
        let entries = ids
            .map { TrashedRidesFile.Entry(id: $0.key.rawValue, trashedAt: $0.value) }
            .sorted { $0.id < $1.id }
        write(TrashedRidesFile(version: Self.schemaVersion, entries: entries), to: trashedURL)
    }

    // MARK: Paths + IO

    private static let schemaVersion = 1
    /// The summary manifest and complete canonical point records share one version.
    private static let rideSchemaVersion = 3
    /// Trips version independently of planned routes (the `rideSchemaVersion`
    /// precedent) — used on **both** the write and the read side, so a future
    /// planned-route bump can't silently stop stored trips from loading.
    fileprivate static let tripSchemaVersion = 1

    private var plannedDir: URL { directory.appendingPathComponent("planned", isDirectory: true) }
    private var tripsDir: URL { directory.appendingPathComponent("trips", isDirectory: true) }
    private var ridesDir: URL { directory.appendingPathComponent("rides", isDirectory: true) }
    private var syncedURL: URL { directory.appendingPathComponent("synced-rides.json") }
    private var deletedURL: URL { directory.appendingPathComponent("deleted-rides.json") }
    private var trashedURL: URL { directory.appendingPathComponent("trashed-rides.json") }

    private func rideDir(_ id: RideID) -> URL {
        ridesDir.appendingPathComponent(Self.fileSafe(id.rawValue), isDirectory: true)
    }

    /// The sidecar keeps the original extension so a saved GPX/TCX stays
    /// recognizable on disk; the exact original name lives in the JSON.
    private static func sourceName(for fileName: String) -> String {
        let ext = (fileName as NSString).pathExtension.lowercased()
        let safe = ext.unicodeScalars.allSatisfy { CharacterSet.alphanumerics.contains($0) }
        return "source." + (ext.isEmpty || !safe ? "bin" : ext)
    }

    /// Injective file-name encoding for ids (`%` is never passed through, so
    /// escapes can't collide with a literal).
    private static func fileSafe(_ raw: String) -> String {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-._"))
        return raw.unicodeScalars
            .map { allowed.contains($0) ? String($0) : "%\(String($0.value, radix: 16, uppercase: true))" }
            .joined()
    }

    private func contents(of dir: URL) -> [URL] {
        (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
    }

    private func ensure(_ dir: URL) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    private func read<T: Decodable>(_ url: URL) -> T? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return try? decoder.decode(T.self, from: data)
    }

    /// Small metadata files stay pretty-printed (diffable, debuggable); the bulky
    /// points file passes `[.sortedKeys]` alone — compact is roughly a third of
    /// the pretty size on a real tracklog.
    private func write<T: Encodable>(
        _ value: T, to url: URL,
        formatting: JSONEncoder.OutputFormatting = [.prettyPrinted, .sortedKeys]
    ) {
        guard let data = try? encode(value, formatting: formatting) else { return }
        try? data.write(to: url, options: .atomic)
    }

    private func encode<T: Encodable>(
        _ value: T, formatting: JSONEncoder.OutputFormatting = [.prettyPrinted, .sortedKeys]
    ) throws -> Data {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        encoder.outputFormatting = formatting
        return try encoder.encode(value)
    }
}

// MARK: - On-disk schema (planned v1, rides v3)

// DTOs, not Codable on the domain types: the file shape is pinned here, so a
// domain refactor can't silently re-shape saved libraries.

private struct PlannedRouteFile: Codable {
    var version: Int
    var summary: RouteSummaryDTO
    var route: ImportedRouteDTO
    var sourceFileName: String
    /// The device object id this route is stored under, `nil` when not on the
    /// device. Optional-decoded, so a pre-B13 file (which lacked it) loads as
    /// "not uploaded" and self-heals on the next upload/reconcile. Stays a bare
    /// `UInt64` on disk (the domain's `DeviceObjectID` wraps it at the
    /// boundary) — no schema bump for #359.
    var deviceObjectID: UInt64?
    /// A device link needs its object id, serial, and store identity.
    /// Incomplete links remain unbound until content reconciliation.
    var deviceSerial: String?
    var deviceStoreID: String?
    /// The committed upload payload's CRC-32 (the `OnDeviceState` fingerprint).
    /// Optional-decoded: a pre-fingerprint file loads as "content unknown",
    /// which reads as outdated and self-heals on the next upload.
    var uploadedCRC32: UInt32?
    /// The **desired** app-side retention level (epic #638) — a bare `u8` on disk,
    /// wrapped into the domain's `Retention` at the boundary. **Optional-decoded:
    /// a pre-expiry file loads as `nil`** (= "not set", pushes nothing — invariant
    /// 6, no surprise deletes). Additive; no schema bump (unknown keys are skipped).
    var retention: UInt8?
    /// Device truth from the last reconcile — display-only, additive/optional.
    /// Kept across launches so the detail screen shows a plausible expiry before
    /// the first reconnect reconcile refreshes it; `nil` on a pre-expiry file.
    var deviceExpiresAt: Date?
    var deviceRetention: UInt8?
    var addedAt: Date

    init(_ record: PlannedRouteRecord) {
        version = 1
        summary = RouteSummaryDTO(record.summary)
        route = ImportedRouteDTO(record.route)
        sourceFileName = record.sourceFileName
        deviceObjectID = record.deviceLink?.objectID.raw
        deviceSerial = record.deviceLink?.serial
        deviceStoreID = record.deviceLink?.storeID
        uploadedCRC32 = record.uploadedCRC32
        retention = record.retention?.rawValue
        deviceExpiresAt = record.deviceExpiresAt
        deviceRetention = record.deviceRetention?.rawValue
        addedAt = record.addedAt
    }

    func record(sourceFileData: Data) -> PlannedRouteRecord {
        let link: DeviceRouteLink? =
            if let deviceObjectID, let deviceSerial, let deviceStoreID {
                DeviceRouteLink(
                    serial: deviceSerial, storeID: deviceStoreID,
                    objectID: DeviceObjectID(deviceObjectID))
            } else {
                nil
            }
        return PlannedRouteRecord(
            summary: summary.domain,
            route: route.domain,
            sourceFileName: sourceFileName,
            sourceFileData: sourceFileData,
            deviceLink: link,
            uploadedCRC32: uploadedCRC32,
            // A desired level is kept as-set (nil stays nil — pushes nothing); the
            // device fields sanitise an unknown byte to `.never` on read.
            retention: retention.map(Retention.init(safeRawValue:)),
            deviceExpiresAt: deviceExpiresAt,
            deviceRetention: deviceRetention.map(Retention.init(safeRawValue:)),
            addedAt: addedAt
        )
    }
}

/// `trips/<id>.json` (trip schema v1) — a trip's metadata: its name and the
/// ordered stage route ids. Additive schema (a pre-trips library simply has no
/// `trips/` dir, so `trips()` reads zero — no migration). Stage ordering is the
/// file's, i.e. the domain's `stageIDs`, source of truth.
///
/// The device link persists exactly the way `PlannedRouteFile`'s does:
/// `deviceObjectID`/`deviceSerial`/`deviceStoreID` as separate optional
/// fields, **all-or-nothing on read** — a partial/flat link (id without
/// serial/store identity) decodes as **no link at all** (#769: the link is only real
/// when all three parts are present, so it can never light a badge or drive a
/// replace-by-id against the wrong device or era). The id stays a bare `UInt64`
/// on disk (the domain's `DeviceObjectID` wraps it at the boundary); link +
/// fingerprint optional-decoded so a not-yet-uploaded trip loads clean.
private struct TripFile: Codable {
    var version: Int
    var id: String
    var name: String
    var stageIDs: [String]
    var deviceObjectID: UInt64?
    var deviceSerial: String?
    var deviceStoreID: String?
    var uploadedCRC32: UInt32?
    var addedAt: Date

    init(_ record: TripRecord) {
        version = FileLibraryStore.tripSchemaVersion
        id = record.id.rawValue
        name = record.name
        stageIDs = record.stageIDs.map(\.rawValue)
        deviceObjectID = record.deviceLink?.objectID.raw
        deviceSerial = record.deviceLink?.serial
        deviceStoreID = record.deviceLink?.storeID
        uploadedCRC32 = record.uploadedCRC32
        addedAt = record.addedAt
    }

    var record: TripRecord {
        let link: DeviceRouteLink? =
            if let deviceObjectID, let deviceSerial, let deviceStoreID {
                DeviceRouteLink(
                    serial: deviceSerial, storeID: deviceStoreID,
                    objectID: DeviceObjectID(deviceObjectID))
            } else {
                nil
            }
        return TripRecord(
            id: TripID(id),
            name: name,
            stageIDs: stageIDs.map(RouteID.init),
            deviceLink: link,
            uploadedCRC32: uploadedCRC32,
            addedAt: addedAt
        )
    }
}

private struct RouteSummaryDTO: Codable {
    var id: String
    var name: String
    var distanceMeters: Double
    var elevationGainMeters: Double
    var estimatedDuration: Double?
    var pointCount: Int
    var source: String?
    var preview: TrackPreviewDTO?

    init(_ summary: RouteSummary) {
        id = summary.id.rawValue
        name = summary.name
        distanceMeters = summary.distanceMeters
        elevationGainMeters = summary.elevationGainMeters
        estimatedDuration = summary.estimatedDuration
        pointCount = summary.pointCount
        source = switch summary.source {
        case .gpx: "gpx"
        case .tcx: "tcx"
        case nil: nil
        }
        preview = summary.trackPreview.map(TrackPreviewDTO.init)
    }

    var domain: RouteSummary {
        let routeSource: RouteSource? = switch source {
        case "gpx": .gpx
        case "tcx": .tcx
        default: nil
        }
        return RouteSummary(
            id: RouteID(id),
            name: name,
            distanceMeters: distanceMeters,
            elevationGainMeters: elevationGainMeters,
            estimatedDuration: estimatedDuration,
            pointCount: pointCount,
            source: routeSource,
            trackPreview: preview?.domain
        )
    }
}

private struct TrackPreviewDTO: Codable {
    /// `[x, y]` pairs in unit space — compact for the ~256-point polylines.
    var points: [[Double]]
    var aspectRatio: Double
    /// `[lat, lon]` pairs, index-aligned with `points` — the source geography the
    /// MapKit basemap preview draws (#294). Optional-decoded: a pre-#294 file
    /// lacked it, so it loads with no coordinates and the preview falls back to
    /// the grid until the record is re-saved.
    var coordinates: [[Double]]?

    init(_ preview: TrackPreview) {
        points = preview.points.map { [$0.x, $0.y] }
        aspectRatio = preview.aspectRatio
        coordinates = preview.coordinates.map { [$0.latitude, $0.longitude] }
    }

    var domain: TrackPreview {
        TrackPreview(
            points: points.compactMap { $0.count == 2 ? TrackPreview.Point(x: $0[0], y: $0[1]) : nil },
            aspectRatio: aspectRatio,
            coordinates: (coordinates ?? []).compactMap {
                $0.count == 2 ? Coordinate(latitude: $0[0], longitude: $0[1]) : nil
            }
        )
    }
}

private struct ImportedRouteDTO: Codable {
    var name: String?
    var creator: String?
    /// `[lat, lon]` or `[lat, lon, ele]` per point.
    var points: [[Double]]
    var waypoints: [WaypointDTO]

    init(_ route: ImportedRoute) {
        name = route.name
        creator = route.creator
        points = route.points.map { point in
            let base = [point.coordinate.latitude, point.coordinate.longitude]
            return point.elevationMeters.map { base + [$0] } ?? base
        }
        waypoints = route.waypoints.map(WaypointDTO.init)
    }

    var domain: ImportedRoute {
        ImportedRoute(
            name: name,
            creator: creator,
            points: points.compactMap { values in
                guard values.count >= 2 else { return nil }
                return RoutePoint(
                    coordinate: Coordinate(latitude: values[0], longitude: values[1]),
                    elevationMeters: values.count >= 3 ? values[2] : nil
                )
            },
            waypoints: waypoints.map(\.domain)
        )
    }
}

private struct WaypointDTO: Codable {
    var index: Int
    var name: String
    var note: String?
    var distanceAlongMeters: Double
    var lat: Double
    var lon: Double
    /// The §7.4 category wire id (`0`/absent = generic) and the signed lateral
    /// offset, both **optional** so a library written before OBCR v3 still decodes
    /// — an older record simply reads back generic and on-route, and re-uploading
    /// it re-derives nothing (the import is where those are fixed).
    var category: UInt8?
    var lateralOffsetMeters: Double?

    init(_ waypoint: Waypoint) {
        index = waypoint.index
        name = waypoint.name
        note = waypoint.note
        distanceAlongMeters = waypoint.distanceAlongMeters
        lat = waypoint.coordinate.latitude
        lon = waypoint.coordinate.longitude
        category = waypoint.category?.rawValue
        lateralOffsetMeters = waypoint.lateralOffsetMeters
    }

    var domain: Waypoint {
        Waypoint(
            index: index, name: name, note: note,
            distanceAlongMeters: distanceAlongMeters,
            coordinate: Coordinate(latitude: lat, longitude: lon),
            category: category.flatMap(WaypointCategory.init(wireID:)),
            lateralOffsetMeters: lateralOffsetMeters ?? 0
        )
    }
}

private struct RideSummaryFile: Codable {
    var version = 3
    var summary: RideSummaryDTO
    var pointsFile: String
    var pointsLength: Int
    var pointsCRC32: UInt32
    var downloaded: Bool

    init(_ summary: RideSummary, pointsFile: String, pointsLength: Int,
         pointsCRC32: UInt32, downloaded: Bool) {
        self.summary = RideSummaryDTO(summary)
        self.pointsFile = pointsFile
        self.pointsLength = pointsLength
        self.pointsCRC32 = pointsCRC32
        self.downloaded = downloaded
    }

    var validPointsName: Bool {
        pointsFile.hasPrefix("points-") && Self.isTransactionFile(pointsFile)
    }

    static func isTransactionFile(_ name: String) -> Bool {
        for prefix in ["points-", "manifest-"] where name.hasPrefix(prefix) && name.hasSuffix(".json") {
            return UUID(uuidString: String(name.dropFirst(prefix.count).dropLast(5))) != nil
        }
        return false
    }
}

private struct RidePointsFile: Codable {
    var version = 3
    var points: [RidePointDTO]
    init(_ points: [RidePoint]) { self.points = points.map(RidePointDTO.init) }
}

private struct RidePointDTO: Codable {
    var timestamp: Date
    var latitude: Double
    var longitude: Double
    var elevation: Double?
    var heartRate: Int?
    var cadence: Int?
    var power: Int?
    var segmentStart: Bool

    init(_ point: RidePoint) {
        timestamp = point.timestamp
        latitude = point.coordinate.latitude
        longitude = point.coordinate.longitude
        elevation = point.elevationMeters
        heartRate = point.heartRate
        cadence = point.cadence
        power = point.power
        segmentStart = point.segmentStart
    }

    var domain: RidePoint {
        RidePoint(timestamp: timestamp, coordinate: Coordinate(latitude: latitude, longitude: longitude),
                  elevationMeters: elevation, heartRate: heartRate, cadence: cadence,
                  power: power, segmentStart: segmentStart)
    }
}

private struct RideSummaryDTO: Codable {
    var source: RideSource?
    var id: String
    var name: String
    var date: Date
    var distanceMeters: Double
    var movingTime: Double
    var averageSpeedMps: Double
    var climbMeters: Double
    var preview: TrackPreviewDTO?
    // Per-ride BLE-sensor summary (ride object v3 footer) — optional, so a
    // pre-#707 `summary.json` (written without these keys) still decodes with
    // every field nil.
    var avgHeartRate: Int?
    var maxHeartRate: Int?
    var avgCadence: Int?
    var avgPower: Int?
    var maxPower: Int?

    init(_ summary: RideSummary) {
        source = summary.source
        id = summary.id.rawValue
        name = summary.name
        date = summary.date
        distanceMeters = summary.distanceMeters
        movingTime = summary.movingTime
        averageSpeedMps = summary.averageSpeedMps
        climbMeters = summary.climbMeters
        preview = summary.trackPreview.map(TrackPreviewDTO.init)
        avgHeartRate = summary.avgHeartRate
        maxHeartRate = summary.maxHeartRate
        avgCadence = summary.avgCadence
        avgPower = summary.avgPower
        maxPower = summary.maxPower
    }

    var domain: RideSummary {
        RideSummary(
            id: RideID(id), name: name, date: date,
            distanceMeters: distanceMeters, movingTime: movingTime,
            averageSpeedMps: averageSpeedMps, climbMeters: climbMeters,
            trackPreview: preview?.domain,
            avgHeartRate: avgHeartRate, maxHeartRate: maxHeartRate,
            avgCadence: avgCadence, avgPower: avgPower, maxPower: maxPower, source: source
        )
    }
}

private struct SyncedRidesFile: Codable {
    var version: Int
    var ids: [String]
}

/// `trashed-rides.json` — the Recently Deleted set (#292): which ride ids are
/// in the trash and when each landed there (the retention purge's clock).
private struct TrashedRidesFile: Codable {
    struct Entry: Codable {
        var id: String
        var trashedAt: Date
    }

    var version: Int
    var entries: [Entry]
}
