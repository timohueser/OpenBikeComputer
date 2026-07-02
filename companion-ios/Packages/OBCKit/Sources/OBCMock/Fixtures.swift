#if DEBUG
import Foundation
import OBCDomain

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
    public var diagnostics: Data

    public init(deviceInfo: DeviceInfo, config: DeviceConfig, battery: Int,
                routes: [RouteEntry], rides: [RideEntry], diagnostics: Data) {
        self.deviceInfo = deviceInfo
        self.config = config
        self.battery = battery
        self.routes = routes
        self.rides = rides
        self.diagnostics = diagnostics
    }
}

/// A fixture route: the enumerable `summary` (with a normalized preview), its
/// `waypoints`, the detail-screen elevation data, and the declared upload payload
/// size — the payload bytes are synthesized on demand (see `blob()`), so a
/// multi-MB library stays cheap to hold.
public struct RouteEntry: Sendable {
    public var summary: RouteSummary
    public var waypoints: [Waypoint]
    public var elevationProfile: [Double]
    public var maxGradePercent: Double?
    public var payloadByteCount: Int

    public init(
        summary: RouteSummary,
        waypoints: [Waypoint] = [],
        elevationProfile: [Double] = [],
        maxGradePercent: Double? = nil,
        payloadByteCount: Int
    ) {
        self.summary = summary
        self.waypoints = waypoints
        self.elevationProfile = elevationProfile
        self.maxGradePercent = maxGradePercent
        self.payloadByteCount = payloadByteCount
    }

    /// The full uploadable route, with a deterministic synthesized payload.
    public func blob() -> RouteBlob {
        RouteBlob(summary: summary, waypoints: waypoints, payload: MockPayload.make(count: payloadByteCount))
    }

    /// What `routeDetail(_:)` serves for this route (E2).
    public func detail() -> RouteDetail {
        RouteDetail(
            summary: summary, waypoints: waypoints,
            elevationProfile: elevationProfile, maxGradePercent: maxGradePercent
        )
    }
}

/// A fixture ride: the enumerable `summary`, its elevation profile (E3), and its
/// declared download size (used to pace `downloadRides` progress).
public struct RideEntry: Sendable {
    public var summary: RideSummary
    public var elevationProfile: [Double]
    public var downloadByteCount: Int

    public init(summary: RideSummary, elevationProfile: [Double] = [], downloadByteCount: Int? = nil) {
        self.summary = summary
        self.elevationProfile = elevationProfile
        // Tracklogs are chunkier than routes; ~20 B/m gives a believable sync size.
        self.downloadByteCount = downloadByteCount ?? max(1, Int(summary.distanceMeters) * 20)
    }

    /// What `rideDetail(_:)` serves for this ride (E3).
    public func detail() -> RideDetail {
        RideDetail(summary: summary, elevationProfile: elevationProfile)
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

    /// Minimal safety net when no JSON is present (keeps the mock alive without resources).
    public static let builtIn = FixtureSet(
        deviceInfo: DeviceInfo(name: "OBC (mock)", firmwareVersion: "0.0.0-mock"),
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
        case gpx, tcx, bad
    }

    public static func fileName(_ kind: Kind = .gpx) -> String {
        switch kind {
        case .gpx, .tcx: "sample-import.\(kind.rawValue)"
        case .bad: "packing-list.pdf"
        }
    }

    public static func data(_ kind: Kind = .gpx) -> Data? {
        switch kind {
        case .gpx, .tcx:
            Bundle.module.url(forResource: "sample-import", withExtension: kind.rawValue)
                .flatMap { try? Data(contentsOf: $0) }
        case .bad:
            Data("socks · stove · sleeping bag — definitely not a route\n".utf8)
        }
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

    var fixtureSet: FixtureSet {
        FixtureSet(
            deviceInfo: deviceInfo.domain,
            config: config.domain,
            battery: battery,
            routes: routes.map(\.entry),
            rides: rides.map(\.entry),
            diagnostics: Data((diagnostics ?? "").utf8)
        )
    }
}

private struct DeviceInfoDTO: Decodable {
    let name: String
    let firmwareVersion: String
    let hardwareVersion: String?
    let serial: String?
    let protocolVersion: UInt16?

    var domain: DeviceInfo {
        DeviceInfo(name: name, firmwareVersion: firmwareVersion,
                   hardwareVersion: hardwareVersion ?? "", serial: serial ?? "",
                   protocolVersion: protocolVersion ?? OBCProtocol.version)
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
        return RouteEntry(summary: summary, waypoints: wps,
                          elevationProfile: track.compactMap(\.ele),
                          maxGradePercent: maxGradePercent,
                          payloadByteCount: payloadBytes ?? max(1, Int(distanceMeters)))
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
        return RideEntry(summary: summary, elevationProfile: track.compactMap(\.ele),
                         downloadByteCount: payloadBytes)
    }
}
#endif
