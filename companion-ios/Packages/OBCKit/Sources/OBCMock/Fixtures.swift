#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// A loaded fixture set — the domain objects the mock serves. Value type so the
/// live `MockControl` can copy-mutate it (delete a route, rename, add a ride) under
/// its lock. Built by decoding editable JSON in `OBCMock/Fixtures/` (`default`,
/// `empty`, `large`), or the tiny `builtIn` fallback if a file is missing.
public struct FixtureSet: Sendable {
    public var deviceInfo: DeviceInfo
    public var config: DeviceConfig
    public var battery: Int
    public var routes: [RouteEntry]
    public var rides: [RideEntry]
    /// Trips grouping some of `routes` (TR6) — seeded into the library as phone
    /// records (a trip is app metadata; the device knows nothing of it until an
    /// upload). Empty in every fixture set but the trips demo.
    public var trips: [TripEntry]
    public var diagnostics: Data

    public init(deviceInfo: DeviceInfo, config: DeviceConfig, battery: Int,
                routes: [RouteEntry], rides: [RideEntry], trips: [TripEntry] = [],
                diagnostics: Data) {
        self.deviceInfo = deviceInfo
        self.config = config
        self.battery = battery
        self.routes = routes
        self.rides = rides
        self.trips = trips
        self.diagnostics = diagnostics
    }
}

/// A fixture trip — the app-side grouping of member routes by their **library**
/// id, in ride order (TR6). Seeded into the mock run's `LibraryStore` as a
/// `TripRecord`; carries no device link (a trip lands on the device only via
/// TR8's whole-trip upload). `order` fixes its `addedAt` so it interleaves with
/// the loose route cards deterministically.
public struct TripEntry: Sendable {
    public var id: TripID
    public var name: String
    public var stageIDs: [RouteID]
    /// Seconds subtracted from the seed base date (bigger = older) — the trip's
    /// slot in the newest-first list among the seeded routes.
    public var order: Double

    public init(id: TripID, name: String, stageIDs: [RouteID], order: Double = 0) {
        self.id = id
        self.name = name
        self.stageIDs = stageIDs
        self.order = order
    }

    /// The library record this fixture seeds — a phone-local trip (no device link).
    public func record(base: Date) -> TripRecord {
        TripRecord(id: id, name: name, stageIDs: stageIDs, addedAt: base.addingTimeInterval(-order))
    }
}

/// A fixture route — a **library-saved planned route** (the app's Planned list is
/// library-first, #289): the list `summary` (with a normalized preview), the
/// parsed geometry (`points` + `waypoints`), the detail-screen elevation data,
/// and the declared upload payload size (payload bytes are synthesized on demand,
/// see `blob()`, so a multi-MB library stays cheap to hold).
///
/// `deviceObjectID` marks the routes the device also holds a copy of: they show
/// the C1 "on device" badge, and `MockTransport.listRoutes()` serves exactly this
/// subset — as `RouteCatalogEntry` values keyed by that id — the way the real
/// device's `routeList` would.
public struct RouteEntry: Sendable {
    public var summary: RouteSummary
    public var points: [RoutePoint]
    public var waypoints: [Waypoint]
    public var elevationProfile: [Double]
    public var maxGradePercent: Double?
    public var payloadByteCount: Int
    /// The device object id this route is stored under on the (mock) device, or
    /// `nil` when it lives only in the phone's library.
    public var deviceObjectID: DeviceObjectID?
    /// The whole-object CRC-32 the (mock) device reports for this copy in its
    /// v2 `routeList` (spec §7.4) — the proof half of the app's identity-verified
    /// badge (#770). `nil` = "derive it from the fixture geometry" (what a seeded
    /// copy's committed CRC is); a real upload pins the committed payload's CRC
    /// here so a re-listed copy proves against the same fingerprint.
    public var crc32: UInt32?
    /// The retention level the (mock) **device** reports for this copy in its v2
    /// `routeList` (epic #638), when the device holds it (`deviceObjectID != nil`).
    /// `nil` → the device serves `.never` (invariant 6 — a pre-existing route
    /// migrates as Never). Only meaningful with a `deviceObjectID`.
    public var deviceRetention: Retention?
    /// How long ago (in days) the device last used this route — the `last_used`
    /// anchor the `expires_at` countdown runs from. `nil` → no anchor (the device
    /// reports `expires_at = 0`). A near-expiry fixture pairs a short retention
    /// with a `lastUsedDaysAgo` close to it (`oneWeek` + `5` → expires in ~2 d) so
    /// S7 can exercise the ≤ 3-day list badge.
    public var lastUsedDaysAgo: Double?

    public init(
        summary: RouteSummary,
        points: [RoutePoint] = [],
        waypoints: [Waypoint] = [],
        elevationProfile: [Double] = [],
        maxGradePercent: Double? = nil,
        payloadByteCount: Int,
        deviceObjectID: DeviceObjectID? = nil,
        crc32: UInt32? = nil,
        deviceRetention: Retention? = nil,
        lastUsedDaysAgo: Double? = nil
    ) {
        self.summary = summary
        self.points = points
        self.waypoints = waypoints
        self.elevationProfile = elevationProfile
        self.maxGradePercent = maxGradePercent
        self.payloadByteCount = payloadByteCount
        self.deviceObjectID = deviceObjectID
        self.crc32 = crc32
        self.deviceRetention = deviceRetention
        self.lastUsedDaysAgo = lastUsedDaysAgo
    }

    /// The full uploadable route, with a deterministic synthesized payload.
    public func blob() -> RouteBlob {
        RouteBlob(summary: summary, waypoints: waypoints, payload: MockPayload.make(count: payloadByteCount))
    }

    /// What the device serves for this route (E2 detail / list reconcile).
    public func detail() -> RouteDetail {
        RouteDetail(
            summary: summary, waypoints: waypoints,
            elevationProfile: elevationProfile, maxGradePercent: maxGradePercent
        )
    }

    /// The library record this fixture seeds (B1S) — what the composition root
    /// writes into the mock run's `InMemoryLibraryStore` so scenarios boot with a
    /// populated, library-first Planned list. `addedAt` fixes the list order
    /// (newest first, so pass descending dates for stable fixture order).
    /// `scope` is the mock device's (serial, epoch) identity (#769): a fixture
    /// the device holds seeds a fully scoped `deviceLink` — passing `nil`
    /// (an identity-less mock) seeds no link at all, mirroring how a v1 flat
    /// link decodes to no link.
    public func record(addedAt: Date, scope: LibraryScope? = nil) -> PlannedRouteRecord {
        let link: DeviceRouteLink? =
            if let deviceObjectID, let scope {
                DeviceRouteLink(serial: scope.serial, epoch: scope.epoch, objectID: deviceObjectID)
            } else {
                nil
            }
        return PlannedRouteRecord(
            summary: summary,
            route: ImportedRoute(name: summary.name, points: points, waypoints: waypoints),
            sourceFileName: "\(summary.id.rawValue).gpx",
            sourceFileData: Data(),
            deviceLink: link,
            addedAt: addedAt
        )
    }
}

/// A fixture ride: the enumerable `summary`, its tracklog (E3 + the B7 download
/// payload), and its declared download size (used to pace `downloadRides`
/// progress — a fiction independent of the payload; the mock's realism is
/// timing + faults, not byte counts).
public struct RideEntry: Sendable {
    public var summary: RideSummary
    public var points: [RidePoint]
    public var elevationProfile: [Double]
    public var downloadByteCount: Int

    public init(
        summary: RideSummary,
        points: [RidePoint] = [],
        elevationProfile: [Double] = [],
        downloadByteCount: Int? = nil
    ) {
        self.summary = summary
        self.points = points
        self.elevationProfile = elevationProfile
        // Tracklogs are chunkier than routes; ~20 B/m gives a believable sync size.
        self.downloadByteCount = downloadByteCount ?? max(1, Int(summary.distanceMeters) * 20)
    }

    /// What `rideDetail(_:)` serves for this ride (E3).
    public func detail() -> RideDetail {
        RideDetail(summary: summary, elevationProfile: elevationProfile)
    }

    /// The canonical full ride — what `downloadRides` encodes into the payload
    /// (via `RideObjectCodec`), so a sync exercises the real decode path.
    public func ride() -> Ride {
        Ride(summary: summary, points: points)
    }
}

// MARK: - Loading

extension FixtureSet {
    /// Decode a bundled fixture set by name (`default`, `empty`, `large`). Falls back
    /// to `builtIn` if the resource is missing or unreadable — the mock never traps.
    public static func load(_ named: String) -> FixtureSet {
        guard
            let url = Bundle.module.url(forResource: named, withExtension: "json"),
            let data = try? Data(contentsOf: url)
        else { return .builtIn }

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        guard let file = try? decoder.decode(FixtureFile.self, from: data) else { return .builtIn }
        return file.fixtureSet
    }

    /// The store epoch every mock device reports unless a fixture overrides it
    /// (#769). Any stable value works — the mock never resets its id era on
    /// its own — but it must be **present**: the app's identity gate is
    /// fail-closed, so an epoch-less mock would boot every scenario with sync
    /// and the possession ack dead.
    public static let defaultStoreEpoch: UInt32 = 0x0BC0_0001

    /// The OBCM map-format version every mock device reports unless a fixture
    /// overrides it (E1 / #911) — what the reference firmware's reader reads
    /// (`obc_formats::obcm::VERSION`). A mock device is a device, so it states
    /// one rather than serving the pre-E1 short read by default.
    public static let defaultObcmVersion: UInt8 = 14

    /// The optional contracts a mock device announces unless a fixture overrides them (WX3 §11):
    /// current firmware implements weather, so the mock does too — otherwise every mock run would
    /// show the weather screen's "this OBC has no weather support" state and nothing else.
    public static let defaultFeatureBits: UInt32 = OBCProtocol.featureWeather

    /// Minimal safety net when no JSON is present (keeps the mock alive without resources).
    public static let builtIn = FixtureSet(
        deviceInfo: DeviceInfo(
            name: "OBC (mock)", firmwareVersion: "0.0.0-mock",
            serial: "OBC-MOCK-000000", storeEpoch: defaultStoreEpoch,
            obcmVersion: defaultObcmVersion, featureBits: defaultFeatureBits),
        config: DeviceConfig(name: "OBC (mock)"),
        battery: 72, routes: [], rides: [],
        diagnostics: Data("OBC diagnostics — built-in fallback\n".utf8)
    )
}

/// The bundled sample route files the `-OBCImportSample` launch hook feeds the
/// import path, so E1/H4/H5 demos and XCUITests exercise the same decoder a
/// Files pick does. `gpx` is a real Komoot export (Schwarzwald tour,
/// downsampled), `tcx` a Garmin-style course (Alpe d'Huez), and `bad` the
/// design's I2 impostor — a PDF name over non-route bytes, for H5.
public enum SampleRouteFile {
    /// Raw values are the `-OBCImportSample <kind>` launch tokens.
    public enum Kind: String, Sendable {
        case gpx, tcx, bad, grimsel
    }

    public static func fileName(_ kind: Kind = .gpx) -> String {
        switch kind {
        case .gpx, .tcx: "sample-import.\(kind.rawValue)"
        case .grimsel: "website-import.gpx"
        case .bad: "packing-list.pdf"
        }
    }

    public static func data(_ kind: Kind = .gpx) -> Data? {
        switch kind {
        case .gpx, .tcx:
            Bundle.module.url(forResource: "sample-import", withExtension: kind.rawValue)
                .flatMap { try? Data(contentsOf: $0) }
        case .grimsel:
            Bundle.module.url(forResource: "website-import", withExtension: "gpx")
                .flatMap { try? Data(contentsOf: $0) }
        case .bad:
            Data("socks · stove · sleeping bag — definitely not a route\n".utf8)
        }
    }
}

/// A synthetic OBCU v2 update container (`OBCU_Spec.md` §1) for the
/// `-OBCFirmwareDemo` launch hook and previews — the Files picker can't be driven
/// from automation, so a demo/screenshot run needs a pre-staged file. Both CRCs
/// are correct and the signature marker is set, so `StagedFirmware.validate` accepts
/// it just like a real `UPDATE.BIN`. Not a real image — the raw body is a
/// deterministic pattern.
///
/// **Its signature is a placeholder.** The app deliberately does not verify signatures
/// (the trusted key lives in the firmware — §1.4), so a demo fixture only needs a
/// well-formed 64-byte trailer to exercise every app-side path. A real device would
/// refuse this file at the arm, which is exactly correct: it isn't a real release.
public enum SampleFirmwareFile {
    /// A ~0.9 MB container tagged `version`, sized to feel like a real firmware
    /// image so the transfer bar paces realistically.
    public static func container(version: String = "0.5.0", imageBytes: Int = 900_000) -> Data {
        var image = Data(capacity: imageBytes)
        image.append(contentsOf: withUnsafeBytes(of: UInt32(0x2002_0000).littleEndian, Array.init))
        image.append(contentsOf: (4..<imageBytes).map { UInt8($0 & 0xFF) })

        var header = Data(count: 64)
        header.replaceSubrange(0..<4, with: Array("OBCU".utf8))
        header[4] = 1 // header_version LE — still 1 in a v2 container (§1.2)
        header.replaceSubrange(8..<12, with: withUnsafeBytes(of: UInt32(image.count).littleEndian, Array.init))
        header.replaceSubrange(12..<16, with: withUnsafeBytes(of: CRC32.checksum(image).littleEndian, Array.init))
        let v = Array(version.utf8.prefix(32))
        header.replaceSubrange(16..<16 + v.count, with: v)
        // 48..52: sig_scheme = 1 (Ed25519), sig_len = 64 — the v2 marker (§1.1).
        header.replaceSubrange(48..<50, with: withUnsafeBytes(of: UInt16(1).littleEndian, Array.init))
        header.replaceSubrange(50..<52, with: withUnsafeBytes(of: UInt16(64).littleEndian, Array.init))
        header.replaceSubrange(60..<64, with: withUnsafeBytes(of: CRC32.checksum(header[0..<60]).littleEndian, Array.init))
        // A deterministic stand-in trailer (see the note above — not a valid signature).
        let signature = Data((0..<64).map { UInt8(($0 &* 7 &+ 3) & 0xFF) })
        return header + image + signature
    }
}

/// Deterministic opaque payload bytes — stands in for the compact-binary route/ride
/// object the real path would stream. Cheap to make; the exact bytes don't matter
/// (the mock never frames or CRCs them — see `OBCProtocol.md`).
public enum MockPayload {
    public static func make(count: Int) -> Data {
        guard count > 0 else { return Data() }
        var data = Data(count: count)
        data.withUnsafeMutableBytes { raw in
            let bytes = raw.bindMemory(to: UInt8.self)
            for i in 0..<count { bytes[i] = UInt8((i &* 13 &+ 7) & 0xFF) }
        }
        return data
    }
}

// MARK: - JSON DTOs (editable-fixture shape → domain)

/// Top-level fixture file. Kept separate from `FixtureSet` so the on-disk shape (raw
/// lat/lon tracks, string enums, optional fields) can stay human-editable.
private struct FixtureFile: Decodable {
    let deviceInfo: DeviceInfoDTO
    let config: ConfigDTO
    let battery: Int
    let diagnostics: String?
    let routes: [RouteDTO]
    let rides: [RideDTO]
    /// Optional (only the trips demo fixture carries it) — grouping some of
    /// `routes` into trips by their string ids.
    let trips: [TripDTO]?

    var fixtureSet: FixtureSet {
        FixtureSet(
            deviceInfo: deviceInfo.domain,
            config: config.domain,
            battery: battery,
            routes: routes.map(\.entry),
            rides: rides.map(\.entry),
            trips: (trips ?? []).map(\.entry),
            diagnostics: Data((diagnostics ?? "").utf8)
        )
    }
}

private struct TripDTO: Decodable {
    let id: String
    let name: String
    let stages: [String]
    let order: Double?

    var entry: TripEntry {
        TripEntry(
            id: TripID(id), name: name,
            stageIDs: stages.map(RouteID.init), order: order ?? 0)
    }
}

private struct DeviceInfoDTO: Decodable {
    let name: String
    let firmwareVersion: String
    let hardwareVersion: String?
    let serial: String?
    let protocolVersion: UInt16?
    /// Optional in the JSON; defaults to `FixtureSet.defaultStoreEpoch` so
    /// every fixture device has an id era (#769 — the identity gate is
    /// fail-closed, and a device without an epoch can't sync).
    let storeEpoch: UInt32?
    /// Optional in the JSON; defaults to `FixtureSet.defaultObcmVersion` so a
    /// mock device states the map format it reads, the way a real one does.
    let obcmVersion: UInt8?
    /// Optional in the JSON; defaults to `FixtureSet.defaultFeatureBits` — a mock device is a
    /// *current* device, so it announces the optional contracts current firmware implements
    /// (weather, WX3). `-OBCWeatherDemo unsupported` is how a run models an older one.
    let featureBits: UInt32?

    var domain: DeviceInfo {
        DeviceInfo(name: name, firmwareVersion: firmwareVersion,
                   hardwareVersion: hardwareVersion ?? "", serial: serial ?? "",
                   protocolVersion: protocolVersion ?? OBCProtocol.version,
                   storeEpoch: storeEpoch ?? FixtureSet.defaultStoreEpoch,
                   obcmVersion: obcmVersion ?? FixtureSet.defaultObcmVersion,
                   featureBits: featureBits ?? FixtureSet.defaultFeatureBits)
    }
}

private struct ConfigDTO: Decodable {
    let name: String
    let units: String?

    var domain: DeviceConfig {
        DeviceConfig(name: name, units: units == "imperial" ? .imperial : .metric)
    }
}

private struct GeoDTO: Decodable {
    let lat: Double
    let lon: Double
    /// Elevation in metres — feeds the detail screens' profile card (E2/E3).
    let ele: Double?
    var coordinate: Coordinate { Coordinate(latitude: lat, longitude: lon) }
}

private struct WaypointDTO: Decodable {
    let name: String
    let note: String?
    let distanceAlongMeters: Double
    let lat: Double
    let lon: Double
}

private struct RouteDTO: Decodable {
    let id: String
    let name: String
    let distanceMeters: Double
    let elevationGainMeters: Double
    let estimatedDuration: TimeInterval?
    let source: String?
    let maxGradePercent: Double?
    let payloadBytes: Int?
    /// The device object id when the (mock) device holds a copy — lights the C1
    /// badge and puts the route in `listRoutes()`. Absent = phone-library only.
    /// A bare number in the JSON, wrapped into the domain's `DeviceObjectID`.
    let deviceObjectID: DeviceObjectID?
    /// The device's retention level for this copy (epic #638) — the wire byte
    /// (`0` never … `5` two months). Absent → the device serves `.never`.
    let deviceRetention: UInt8?
    /// The `last_used` anchor as "days ago" — a near-expiry fixture pairs it with
    /// a short `deviceRetention`. Absent → no anchor (`expires_at = 0`).
    let lastUsedDaysAgo: Double?
    let track: [GeoDTO]
    let waypoints: [WaypointDTO]?

    var routeSource: RouteSource? {
        switch source {
        case "gpx": return .gpx
        case "tcx": return .tcx
        default: return nil
        }
    }

    var entry: RouteEntry {
        let summary = RouteSummary(
            id: RouteID(id), name: name,
            distanceMeters: distanceMeters, elevationGainMeters: elevationGainMeters,
            estimatedDuration: estimatedDuration, pointCount: track.count,
            source: routeSource, trackPreview: TrackPreview.normalizing(track.map(\.coordinate))
        )
        let wps = (waypoints ?? []).enumerated().map { index, wp in
            Waypoint(index: index, name: wp.name, note: wp.note,
                     distanceAlongMeters: wp.distanceAlongMeters,
                     coordinate: Coordinate(latitude: wp.lat, longitude: wp.lon))
        }
        return RouteEntry(summary: summary,
                          points: track.map { RoutePoint(coordinate: $0.coordinate, elevationMeters: $0.ele) },
                          waypoints: wps,
                          elevationProfile: track.compactMap(\.ele),
                          maxGradePercent: maxGradePercent,
                          payloadByteCount: payloadBytes ?? max(1, Int(distanceMeters)),
                          deviceObjectID: deviceObjectID,
                          deviceRetention: deviceRetention.map(Retention.init(safeRawValue:)),
                          lastUsedDaysAgo: lastUsedDaysAgo)
    }
}

private struct RideDTO: Decodable {
    let id: String
    let name: String
    let date: Date
    let distanceMeters: Double
    let movingTime: TimeInterval
    let averageSpeedMps: Double
    let climbMeters: Double
    let payloadBytes: Int?
    let track: [GeoDTO]

    var entry: RideEntry {
        let summary = RideSummary(
            id: RideID(id), name: name, date: date, distanceMeters: distanceMeters,
            movingTime: movingTime, averageSpeedMps: averageSpeedMps, climbMeters: climbMeters,
            trackPreview: TrackPreview.normalizing(track.map(\.coordinate))
        )
        // Fixture tracks carry no timestamps — synthesize them evenly across the
        // moving time, so the encoded payload is a plausible recorded tracklog.
        let step = track.count > 1 ? movingTime / Double(track.count - 1) : 0
        let points = track.enumerated().map { index, geo in
            RidePoint(timestamp: date.addingTimeInterval(Double(index) * step),
                      coordinate: geo.coordinate, elevationMeters: geo.ele)
        }
        return RideEntry(summary: summary, points: points,
                         elevationProfile: track.compactMap(\.ele),
                         downloadByteCount: payloadBytes)
    }
}
#endif
