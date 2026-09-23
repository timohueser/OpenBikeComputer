#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// A loaded fixture set: the domain objects the mock serves. A value type, so the live
/// `MockControl` can copy-mutate it under its lock. Built by decoding editable JSON in
/// `OBCMock/Fixtures/`, or the tiny `builtIn` fallback if a file is missing.
public struct FixtureSet: Sendable {
    public var deviceInfo: DeviceInfo
    public var config: DeviceConfig
    public var battery: Int
    public var routes: [RouteEntry]
    public var rides: [RideEntry]
    /// Trips grouping some of `routes`, seeded into the library as phone records. A trip is app
    /// metadata; the device knows nothing of it until an upload.
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

/// A fixture trip: fixture routes joined into one line, one day per route, in ride order. The
/// member routes seed only as the trip, never as loose routes, because a route added to a trip
/// becomes part of its line. It carries no device link: a trip lands on the device only through a
/// whole-trip upload. `order` fixes its `addedAt`, so it interleaves with the route cards
/// deterministically.
public struct TripEntry: Sendable {
    public var id: TripID
    public var name: String
    public var routeIDs: [RouteID]
    /// Seconds subtracted from the seed base date; bigger is older. It fixes the trip's slot in
    /// the newest-first list.
    public var order: Double

    public init(id: TripID, name: String, routeIDs: [RouteID], order: Double = 0) {
        self.id = id
        self.name = name
        self.routeIDs = routeIDs
        self.order = order
    }

    /// The library trip this fixture seeds from the fixture routes' geometry.
    public func trip(routes: [RouteEntry], base: Date) -> Trip {
        let members = routeIDs.compactMap { id in routes.first { $0.summary.id == id } }
        return Trip.joining(
            members.map(\.points), names: members.map(\.summary.name), id: id, name: name, bikeType: .road,
            now: base.addingTimeInterval(-order))
    }
}

/// A fixture route: a library-saved planned route, because the Planned list is library-first. It
/// carries the list summary with a normalized preview, the parsed geometry, the detail-screen
/// elevation data, and the declared upload payload size. Payload bytes are synthesized on demand,
/// so a multi-MB library stays cheap to hold.
///
/// `deviceObjectID` marks the routes the device also holds a copy of: they show the "on device"
/// badge, and `MockTransport.listRoutes()` serves exactly that subset, the way the real device's
/// catalog would.
public struct RouteEntry: Sendable {
    public var summary: RouteSummary
    public var points: [RoutePoint]
    public var waypoints: [Waypoint]
    public var elevationProfile: [Double]
    public var maxGradePercent: Double?
    public var payloadByteCount: Int
    /// The device object id this route is stored under on the mock device, or nil when it lives
    /// only in the phone's library.
    public var deviceObjectID: DeviceObjectID?
    /// The whole-object CRC-32 the mock device reports for this copy, the proof half of the app's
    /// identity-verified badge. Nil means derive it from the fixture geometry, which is what a
    /// seeded copy's committed CRC is; a real upload pins the committed payload's CRC here, so a
    /// re-listed copy proves against the same fingerprint.
    public var crc32: UInt32?

    public init(
        summary: RouteSummary,
        points: [RoutePoint] = [],
        waypoints: [Waypoint] = [],
        elevationProfile: [Double] = [],
        maxGradePercent: Double? = nil,
        payloadByteCount: Int,
        deviceObjectID: DeviceObjectID? = nil,
        crc32: UInt32? = nil,
    ) {
        self.summary = summary
        self.points = points
        self.waypoints = waypoints
        self.elevationProfile = elevationProfile
        self.maxGradePercent = maxGradePercent
        self.payloadByteCount = payloadByteCount
        self.deviceObjectID = deviceObjectID
        self.crc32 = crc32
    }

    /// The full uploadable route, with a deterministic synthesized payload.
    public func blob() -> RouteBlob {
        RouteBlob(summary: summary, waypoints: waypoints, payload: MockPayload.make(count: payloadByteCount))
    }

    /// What the device serves for this route.
    public func detail() -> RouteDetail {
        RouteDetail(
            summary: summary, waypoints: waypoints,
            elevationProfile: elevationProfile, maxGradePercent: maxGradePercent
        )
    }

    /// The library record this fixture seeds: what the composition root writes into the mock run's
    /// store, so scenarios boot with a populated, library-first Planned list. `addedAt` fixes the
    /// list order, newest first, so pass descending dates for a stable fixture order. `scope` is
    /// the mock device's identity: a fixture the device holds seeds a fully scoped `deviceLink`,
    /// and passing nil seeds no link at all.
    public func record(addedAt: Date, scope: LibraryScope? = nil) -> PlannedRouteRecord {
        let link: DeviceRouteLink? =
            if let deviceObjectID, let scope {
                DeviceRouteLink(scope: scope, objectID: deviceObjectID)
            } else {
                nil
            }
        // The library keeps the estimate for the record's type, as an import saves it.
        var summary = summary
        summary.estimatedDuration = BikeType.road.estimatedDuration(
            distanceMeters: summary.distanceMeters, ascentMeters: summary.elevationGainMeters)
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

/// A fixture ride: the enumerable summary, its tracklog, and its declared download size. The size
/// paces `downloadRides` progress and is a fiction independent of the payload, because the mock's
/// realism is timing and faults, not byte counts.
public struct RideEntry: Sendable {
    public var summary: RideSummary
    public var points: [RidePoint]
    public var downloadByteCount: Int

    public init(
        summary: RideSummary,
        points: [RidePoint] = [],
        downloadByteCount: Int? = nil
    ) {
        self.summary = summary
        self.points = points
        // Tracklogs are chunkier than routes; ~20 B/m gives a believable sync size.
        self.downloadByteCount = downloadByteCount ?? max(1, Int(summary.distanceMeters) * 20)
    }

    /// The canonical full ride, which `downloadRides` encodes into the payload, so a sync
    /// exercises the real decode path.
    public func ride() -> Ride {
        Ride(summary: summary, points: points)
    }
}

// MARK: - Loading

extension FixtureSet {
    /// Decode a bundled fixture set by name. The ride-only variant reuses its source fixture
    /// without the planned route, so an import can share that launch without a collision. Missing
    /// or unreadable resources fall back to `builtIn`: the mock never traps.
    public static func load(_ named: String) -> FixtureSet {
        if named == "website-rides" {
            var fixtures = load("website")
            fixtures.routes = []
            return fixtures
        }
        guard
            let url = Bundle.module.url(forResource: named, withExtension: "json"),
            let data = try? Data(contentsOf: url)
        else { return .builtIn }

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        guard let file = try? decoder.decode(FixtureFile.self, from: data) else { return .builtIn }
        return file.fixtureSet
    }

    /// Stable full store identity for built-in mock data.
    public static let defaultStoreID = "1111111111111111111111110bc00001"

    /// The map-format version every mock device reports unless a fixture overrides it. A mock
    /// device is a device, so it states one rather than serving a short read by default.
    public static let defaultObcmVersion: UInt8 = 14

    /// Minimal safety net when no JSON is present, which keeps the mock alive without resources.
    public static let builtIn = FixtureSet(
        deviceInfo: DeviceInfo(
            name: "OBC (mock)", firmwareVersion: "0.0.0-mock",
            serial: "OBC-MOCK-000000", storeID: defaultStoreID,
            obcmVersion: defaultObcmVersion),
        config: DeviceConfig(name: "OBC (mock)"),
        battery: 72, routes: [], rides: [],
        diagnostics: Data("OBC diagnostics — built-in fallback\n".utf8)
    )
}

/// The bundled sample route files the import launch hook feeds the import path, so demos and UI
/// tests exercise the same decoder a Files pick does. One is a real GPX export, one a
/// Garmin-style TCX course, and one an impostor: a PDF name over non-route bytes.
public enum SampleRouteFile {
    /// Raw values are the launch-argument tokens.
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

/// A synthetic update container for the firmware demo launch hook and for previews: the Files
/// picker cannot be driven from automation, so a demo run needs a pre-staged file. Both CRCs are
/// correct and the signature marker is set, so validation accepts it just like a real one. It is
/// not a real image: the raw body is a deterministic pattern.
///
/// Its signature is a placeholder. The app deliberately does not verify signatures, because the
/// trusted key lives in the firmware, so a demo fixture only needs a well-formed 64-byte trailer
/// to exercise every app-side path. A real device refuses this file at the install request, which
/// is exactly correct.
public enum SampleFirmwareFile {
    /// A container of about 0.9 MB tagged `version`, sized to feel like a real firmware image so
    /// the transfer bar paces realistically.
    public static func container(version: String = "0.5.0", imageBytes: Int = 900_000) -> Data {
        var image = Data(capacity: imageBytes)
        image.append(contentsOf: withUnsafeBytes(of: UInt32(0x2002_0000).littleEndian, Array.init))
        image.append(contentsOf: (4..<imageBytes).map { UInt8($0 & 0xFF) })

        var header = Data(count: 64)
        header.replaceSubrange(0..<4, with: Array("OBCU".utf8))
        header[4] = 1 // header_version, still 1 in a signed container
        header.replaceSubrange(8..<12, with: withUnsafeBytes(of: UInt32(image.count).littleEndian, Array.init))
        header.replaceSubrange(12..<16, with: withUnsafeBytes(of: CRC32.checksum(image).littleEndian, Array.init))
        let v = Array(version.utf8.prefix(32))
        header.replaceSubrange(16..<16 + v.count, with: v)
        // Bytes 48 to 52 hold the signature scheme and length: the signed-container marker.
        header.replaceSubrange(48..<50, with: withUnsafeBytes(of: UInt16(1).littleEndian, Array.init))
        header.replaceSubrange(50..<52, with: withUnsafeBytes(of: UInt16(64).littleEndian, Array.init))
        header.replaceSubrange(60..<64, with: withUnsafeBytes(of: CRC32.checksum(header[0..<60]).littleEndian, Array.init))
        // A deterministic stand-in trailer, not a valid signature.
        let signature = Data((0..<64).map { UInt8(($0 &* 7 &+ 3) & 0xFF) })
        return header + image + signature
    }
}

/// Deterministic opaque payload bytes: a stand-in for the compact-binary object the real path
/// would stream. Cheap to make, and the exact bytes do not matter, because the mock never frames
/// or checksums them.
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

// MARK: - JSON DTOs

/// Top-level fixture file. Kept separate from `FixtureSet`, so the on-disk shape can stay
/// human-editable.
private struct FixtureFile: Decodable {
    let deviceInfo: DeviceInfoDTO
    let config: ConfigDTO
    let battery: Int
    let diagnostics: String?
    let routes: [RouteDTO]
    let rides: [RideDTO]
    /// Optional, and carried only by the trips demo fixture: it joins some of `routes` into trips
    /// by their string ids.
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
    let routes: [String]
    let order: Double?

    var entry: TripEntry {
        TripEntry(
            id: TripID(id), name: name,
            routeIDs: routes.map(RouteID.init), order: order ?? 0)
    }
}

private struct DeviceInfoDTO: Decodable {
    let name: String
    let firmwareVersion: String
    let hardwareVersion: String?
    let serial: String?
    let protocolVersion: UInt16?
    let storeID: String?
    /// Optional in the JSON; it defaults so a mock device states the map format it reads, the way
    /// a real one does.
    let obcmVersion: UInt8?

    var domain: DeviceInfo {
        DeviceInfo(name: name, firmwareVersion: firmwareVersion,
                   hardwareVersion: hardwareVersion ?? "", serial: serial ?? "",
                   protocolVersion: protocolVersion ?? OBCProtocol.version,
                   storeID: storeID,
                   obcmVersion: obcmVersion ?? FixtureSet.defaultObcmVersion)
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
    /// Elevation in metres, which feeds the detail screens' profile card.
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
    /// The device object id when the mock device holds a copy: it lights the badge and puts the
    /// route in the device catalog. Absent means the route is phone-library only. A bare number in
    /// the JSON, wrapped into the domain type.
    let deviceObjectID: DeviceObjectID?
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
                          deviceObjectID: deviceObjectID)
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
    /// A bike type name in lowercase; absent is Road.
    let bikeType: String?
    /// The trip day the ride started on; absent for a ride without a trip.
    let trip: TripDayDTO?

    struct TripDayDTO: Decodable {
        let key: UInt64
        let dayIndex: Int
        let dayCount: Int
        let name: String
    }

    var entry: RideEntry {
        let summary = RideSummary(
            id: RideID(id), name: name, date: date, distanceMeters: distanceMeters,
            movingTime: movingTime, averageSpeedMps: averageSpeedMps, climbMeters: climbMeters,
            trackPreview: TrackPreview.normalizing(track.map(\.coordinate)),
            bikeType: BikeType.allCases.first { $0.name.lowercased() == bikeType } ?? .road,
            trip: trip.map { RideTrip(key: $0.key, dayIndex: $0.dayIndex, dayCount: $0.dayCount, name: $0.name) }
        )
        // Fixture tracks carry no timestamps — synthesize them evenly across the
        // moving time, so the encoded payload is a plausible recorded tracklog.
        let step = track.count > 1 ? movingTime / Double(track.count - 1) : 0
        let points = track.enumerated().map { index, geo in
            RidePoint(timestamp: date.addingTimeInterval(Double(index) * step),
                      coordinate: geo.coordinate, elevationMeters: geo.ele)
        }
        return RideEntry(summary: summary, points: points, downloadByteCount: payloadBytes)
    }
}
#endif
