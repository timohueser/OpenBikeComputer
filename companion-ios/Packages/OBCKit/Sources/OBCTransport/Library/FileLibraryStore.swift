import Foundation
import OBCDomain

/// The real `LibraryStore`: plain JSON files in an app-owned directory — the
/// "keep it boring" choice (#256; no CoreData/SwiftData until a real need).
///
/// Layout under `directory`:
///
///     planned/<id>/route.json     versioned record (summary + canonical route)
///     planned/<id>/source.<ext>   the original import file, byte-exact
///     trips/<id>.json             versioned trip (name + ordered stage ids), TR5
///     rides/<id>/summary.json     versioned ride summary (the list row)
///     rides/<id>/points.json      versioned tracklog, compact JSON (read on demand)
///     synced-rides.json           every ride id ever downloaded (H9)
///     deleted-rides.json          ride ids deleted on the phone (device keeps its copy)
///     trashed-rides.json          ride ids in Recently Deleted, with trash dates (#292)
///
/// Rides split summary from tracklog (#360, ride schema v2) so launching never
/// decodes a season of points to draw list rows. A v1 whole-ride file
/// (`rides/<id>.json`) migrates lazily: the first read rewrites it split and
/// removes the old file.
///
/// **(serial, epoch) scoping (#769) needs no layout of its own:** every ride
/// path and set entry keys on the `RideID` raw string, and a v2-minted id
/// *is* the `(serial, epoch, id)` composite — so scoped entries land in
/// per-scope directory names (`rides/v2%3A<epoch>%3A<id>%3A<serial>/`) and
/// scoped set ids by construction. An era change or device switch changes the
/// strings; nothing here compares, migrates, or re-keys. Flat v1 entries (bare
/// numeric names) coexist as archival rows until the claim-on-first-contact
/// migration (`LibraryScopeMigrator`) moves the ones a device corroborates.
///
/// The JSON shape is an **app-owned schema** (versioned DTOs below), decoupled
/// from both the domain types' memberwise layout and the device wire formats —
/// a firmware `S0` byte-layout change never touches saved libraries. Unreadable
/// or future-versioned files are skipped, never fatal; writes are best-effort
/// (a full disk loses one save, not the store).
public struct FileLibraryStore: LibraryStore, Sendable {
    private let directory: URL

    /// `directory` is created on first use. Tests point this at a temp dir.
    public init(directory: URL) {
        self.directory = directory
    }

    /// The production location: Application Support, backed up, app-private.
    public static func standard() -> FileLibraryStore {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return FileLibraryStore(directory: base.appendingPathComponent("OBCLibrary", isDirectory: true))
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
        let entries = contents(of: ridesDir)
        // v2 rides are directories; their names key the migration dedupe below.
        let migrated = Set(
            entries.filter(\.hasDirectoryPath).map(\.lastPathComponent))
        return entries
            .compactMap { url -> RideSummary? in
                if url.hasDirectoryPath {
                    guard let file: RideSummaryFile = read(url.appendingPathComponent("summary.json")),
                        file.version == Self.rideSchemaVersion
                    else { return nil }
                    return file.summary.domain
                }
                // A v1 whole-ride file — migrate on first read (#360). If its id
                // already has a split directory (a migration whose old-file
                // removal didn't land), the directory wins: rewriting from the
                // stale file would undo a later rename.
                guard url.pathExtension == "json" else { return nil }
                if migrated.contains(url.deletingPathExtension().lastPathComponent) {
                    try? FileManager.default.removeItem(at: url)
                    return nil
                }
                return migrateLegacyRideFile(at: url)?.summary
            }
            .sorted { $0.date > $1.date }
    }

    public func ridePoints(_ id: RideID) -> [RidePoint]? {
        if let file: RidePointsFile = read(rideDir(id).appendingPathComponent("points.json")),
            file.version == Self.rideSchemaVersion {
            return file.ridePoints
        }
        // Not split yet (a detail opened before any list read) — migrate now.
        return migrateLegacyRideFile(at: legacyRideURL(id))?.points
    }

    public func saveRide(_ ride: Ride) {
        let dir = rideDir(ride.id)
        ensure(dir)
        write(RideSummaryFile(ride.summary), to: dir.appendingPathComponent("summary.json"))
        // The tracklog is the bulky file — compact JSON, written once per sync
        // (or migration) and read one-ride-at-a-time.
        write(RidePointsFile(ride.points), to: dir.appendingPathComponent("points.json"),
              formatting: [.sortedKeys])
        // A re-save of a not-yet-migrated ride must not leave the v1 file to
        // shadow (and later clobber) the split one.
        try? FileManager.default.removeItem(at: legacyRideURL(ride.id))
    }

    public func saveRideSummary(_ summary: RideSummary) {
        // Split a lingering v1 file first, so the points aren't orphaned and a
        // later lazy migration can't overwrite this rename with the stale name.
        _ = migrateLegacyRideFile(at: legacyRideURL(summary.id))
        let dir = rideDir(summary.id)
        ensure(dir)
        write(RideSummaryFile(summary), to: dir.appendingPathComponent("summary.json"))
    }

    public func deleteRide(_ id: RideID) {
        try? FileManager.default.removeItem(at: rideDir(id))
        try? FileManager.default.removeItem(at: legacyRideURL(id))
    }

    /// The lazy v1 → v2 migration (#360): read the whole-ride file, rewrite it
    /// split, remove the original. `nil` (and the file left alone) when it's
    /// unreadable or future-versioned — the skip-not-fatal rule.
    private func migrateLegacyRideFile(at url: URL) -> Ride? {
        guard let file: RideFile = read(url), file.version == 1 else { return nil }
        let ride = file.ride
        saveRide(ride)
        // `saveRide` sweeps `legacyRideURL(ride.id)`; also remove `url` itself in
        // case a hand-moved file's name doesn't match its id — a survivor would
        // re-migrate on every read and clobber later renames.
        try? FileManager.default.removeItem(at: url)
        return ride
    }

    public func syncedRideIDs() -> Set<RideID> {
        guard let file: SyncedRidesFile = read(syncedURL), file.version == Self.schemaVersion
        else { return [] }
        return Set(file.ids.map(RideID.init))
    }

    public func markRideSynced(_ id: RideID) {
        var ids = syncedRideIDs()
        guard ids.insert(id).inserted else { return }
        ensure(directory)
        write(SyncedRidesFile(version: Self.schemaVersion, ids: ids.map(\.rawValue).sorted()), to: syncedURL)
    }

    public func unmarkRideSynced(_ id: RideID) {
        var ids = syncedRideIDs()
        guard ids.remove(id) != nil else { return }
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

    public func unmarkRideDeleted(_ id: RideID) {
        var ids = deletedRideIDs()
        guard ids.remove(id) != nil else { return }
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
    /// Rides split summary/points into separate files (#360); planned routes and
    /// the id sets stay on v1.
    private static let rideSchemaVersion = 2
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

    /// Where the v1 store kept the whole ride — read (and swept) by the migration.
    private func legacyRideURL(_ id: RideID) -> URL {
        ridesDir.appendingPathComponent("\(Self.fileSafe(id.rawValue)).json")
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
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        encoder.outputFormatting = formatting
        guard let data = try? encoder.encode(value) else { return }
        try? data.write(to: url, options: .atomic)
    }
}

// MARK: - On-disk schema (planned v1, rides v2)

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
    /// `UInt16` on disk (the domain's `DeviceObjectID` wraps it at the
    /// boundary) — no schema bump for #359.
    var deviceObjectID: UInt16?
    /// The (serial, epoch) scope of `deviceObjectID` (#769) — additive,
    /// optional-decoded. A **v1 flat file** carries the id alone: it loads as
    /// **no link at all** (the link is only real when all three parts are
    /// present), so a flat link can never light a badge or drive a
    /// replace-by-id upload against the wrong device — V6's CRC adoption is
    /// the path that re-links such records properly. Deliberately *not*
    /// claimed by serial guess in the one-time migration (a mixed two-device
    /// library would mis-attribute the other device's links). No schema bump:
    /// the fields are additive and every decoder skips unknown keys.
    var deviceSerial: String?
    var deviceStoreEpoch: UInt32?
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
        deviceStoreEpoch = record.deviceLink?.epoch
        uploadedCRC32 = record.uploadedCRC32
        retention = record.retention?.rawValue
        deviceExpiresAt = record.deviceExpiresAt
        deviceRetention = record.deviceRetention?.rawValue
        addedAt = record.addedAt
    }

    func record(sourceFileData: Data) -> PlannedRouteRecord {
        let link: DeviceRouteLink? =
            if let deviceObjectID, let deviceSerial, let deviceStoreEpoch {
                DeviceRouteLink(
                    serial: deviceSerial, epoch: deviceStoreEpoch,
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
/// `deviceObjectID`/`deviceSerial`/`deviceStoreEpoch` as separate optional
/// fields, **all-or-nothing on read** — a partial/flat link (id without
/// serial/epoch) decodes as **no link at all** (#769: the link is only real
/// when all three parts are present, so it can never light a badge or drive a
/// replace-by-id against the wrong device or era). The id stays a bare `UInt16`
/// on disk (the domain's `DeviceObjectID` wraps it at the boundary); link +
/// fingerprint optional-decoded so a not-yet-uploaded trip loads clean.
private struct TripFile: Codable {
    var version: Int
    var id: String
    var name: String
    var stageIDs: [String]
    var deviceObjectID: UInt16?
    var deviceSerial: String?
    var deviceStoreEpoch: UInt32?
    var uploadedCRC32: UInt32?
    var addedAt: Date

    init(_ record: TripRecord) {
        version = FileLibraryStore.tripSchemaVersion
        id = record.id.rawValue
        name = record.name
        stageIDs = record.stageIDs.map(\.rawValue)
        deviceObjectID = record.deviceLink?.objectID.raw
        deviceSerial = record.deviceLink?.serial
        deviceStoreEpoch = record.deviceLink?.epoch
        uploadedCRC32 = record.uploadedCRC32
        addedAt = record.addedAt
    }

    var record: TripRecord {
        let link: DeviceRouteLink? =
            if let deviceObjectID, let deviceSerial, let deviceStoreEpoch {
                DeviceRouteLink(
                    serial: deviceSerial, epoch: deviceStoreEpoch,
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

/// `rides/<id>/summary.json` (v2) — the list row, decoded for every ride at launch.
private struct RideSummaryFile: Codable {
    var version: Int
    var summary: RideSummaryDTO

    init(_ summary: RideSummary) {
        version = 2
        self.summary = RideSummaryDTO(summary)
    }
}

/// `rides/<id>/points.json` (v2) — the tracklog, decoded one ride at a time.
private struct RidePointsFile: Codable {
    var version: Int
    /// `[epochSeconds, lat, lon]` or `[epochSeconds, lat, lon, ele]` per sample.
    var points: [[Double]]

    init(_ ridePoints: [RidePoint]) {
        version = 2
        points = ridePoints.map { point in
            let base = [point.timestamp.timeIntervalSince1970,
                        point.coordinate.latitude, point.coordinate.longitude]
            return point.elevationMeters.map { base + [$0] } ?? base
        }
    }

    var ridePoints: [RidePoint] {
        points.compactMap { values in
            guard values.count >= 3 else { return nil }
            return RidePoint(
                timestamp: Date(timeIntervalSince1970: values[0]),
                coordinate: Coordinate(latitude: values[1], longitude: values[2]),
                elevationMeters: values.count >= 4 ? values[3] : nil
            )
        }
    }
}

/// The **v1** whole-ride file (`rides/<id>.json`) — decode-only since #360; the
/// lazy migration's source. Its point rows share `RidePointsFile`'s layout.
private struct RideFile: Decodable {
    var version: Int
    var summary: RideSummaryDTO
    var points: [[Double]]

    var ride: Ride {
        var pointsFile = RidePointsFile([])
        pointsFile.points = points
        return Ride(summary: summary.domain, points: pointsFile.ridePoints)
    }
}

private struct RideSummaryDTO: Codable {
    var id: String
    var name: String
    var date: Date
    var distanceMeters: Double
    var movingTime: Double
    var averageSpeedMps: Double
    var climbMeters: Double
    var preview: TrackPreviewDTO?
    // Per-ride BLE-sensor summary (ride object v2, epic #707) — optional, so a
    // pre-#707 `summary.json` (written without these keys) still decodes with
    // every field nil.
    var avgHeartRate: Int?
    var maxHeartRate: Int?
    var avgCadence: Int?
    var avgPower: Int?
    var maxPower: Int?

    init(_ summary: RideSummary) {
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
            avgCadence: avgCadence, avgPower: avgPower, maxPower: maxPower
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
