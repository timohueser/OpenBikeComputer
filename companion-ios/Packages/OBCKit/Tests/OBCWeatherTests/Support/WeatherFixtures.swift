import Foundation
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// Shared fixture material.
///
/// The grid objects are the **checked-in WX5 vectors** in `specs/vectors`, not bytes this test
/// target invented — the same objects `host/obc-vectors` and `OBCGridCodecTests` pin. The manifests
/// around them are built here from each vector's real header, so a fixture can never drift from the
/// object it describes: if a vector's geometry changed, these manifests would change with it and the
/// client's manifest-versus-header agreement check would still hold.
enum WeatherFixtures {
    static let repositoryRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // Support/
        .deletingLastPathComponent()  // OBCWeatherTests/
        .deletingLastPathComponent()  // Tests/
        .deletingLastPathComponent()  // OBCKit/
        .deletingLastPathComponent()  // Packages/
        .deletingLastPathComponent()  // companion-ios/
        .deletingLastPathComponent()  // repository root

    static func vector(_ name: String) throws -> Data {
        let url = repositoryRoot.appendingPathComponent("specs/vectors").appendingPathComponent(name)
        return try #require(FileManager.default.contents(atPath: url.path), "missing vector \(name)")
    }

    /// The WX1 MET capture: deterministic hourly extracts with their provenance.
    ///
    /// These lived in `host/obc-wx-source-spike/tests/fixtures` until WX6 (#1223) deleted the
    /// spike crate wholesale — orphaning the two captures this suite reads (the deletion PR
    /// touched no `companion-ios/**` path, so the iOS gate never ran on it). They are this
    /// target's own bundled resources now: the spike was disposable, its evidence is not.
    static func metCapture(_ name: String) throws -> METCapture {
        let url = try #require(
            Bundle.module.url(
                forResource: (name as NSString).deletingPathExtension, withExtension: "json",
                subdirectory: "Fixtures"),
            "missing bundled MET capture \(name)")
        let data = try #require(FileManager.default.contents(atPath: url.path), "missing \(name)")
        return try JSONDecoder().decode(METCapture.self, from: data)
    }

    struct METCapture: Decodable {
        struct Provenance: Decodable {
            var latitude: Double
            var longitude: Double
            var altitude_m: Int
            var last_modified: String
            var expires: String
        }

        struct Hour: Decodable {
            var time: String
            var air_temperature_c: Double
            var precipitation_amount_mm: Double
            var probability_of_precipitation_percent: Double?
            var symbol_code: String
            var wind_from_direction_degrees: Double
            var wind_speed_mps: Double
            var wind_gust_mps: Double?
        }

        var provenance: Provenance
        var hours: [Hour]

        /// Re-inflate the provider's own document shape around the captured values.
        ///
        /// The *values* are WX1's real capture; only the envelope is rebuilt, because the capture
        /// deliberately stores a lawful decoded subset rather than MET's full response. Building it
        /// here keeps the adapter's decoder tested against the real field names, units block and
        /// optional-key pattern (Oslo has gust and probability in all 24 hours; Manila has neither
        /// in any).
        func locationforecastJSON(unitOverrides: [String: String] = [:]) -> Data {
            var units: [String: String] = [
                "air_temperature": "celsius", "precipitation_amount": "mm",
                "probability_of_precipitation": "%", "wind_from_direction": "degrees",
                "wind_speed": "m/s", "wind_speed_of_gust": "m/s",
            ]
            for (key, value) in unitOverrides { units[key] = value }

            let series: [[String: Any]] = hours.map { hour in
                var instant: [String: Any] = [
                    "air_temperature": hour.air_temperature_c,
                    "wind_from_direction": hour.wind_from_direction_degrees,
                    "wind_speed": hour.wind_speed_mps,
                ]
                if let gust = hour.wind_gust_mps { instant["wind_speed_of_gust"] = gust }
                var next: [String: Any] = ["precipitation_amount": hour.precipitation_amount_mm]
                if let probability = hour.probability_of_precipitation_percent {
                    next["probability_of_precipitation"] = probability
                }
                return [
                    "time": hour.time,
                    "data": [
                        "instant": ["details": instant],
                        "next_1_hours": [
                            "summary": ["symbol_code": hour.symbol_code],
                            "details": next,
                        ],
                    ],
                ]
            }
            let document: [String: Any] = [
                "type": "Feature",
                "geometry": [
                    "type": "Point",
                    "coordinates": [provenance.longitude, provenance.latitude, provenance.altitude_m],
                ],
                "properties": [
                    "meta": ["updated_at": provenance.last_modified, "units": units],
                    "timeseries": series,
                ],
            ]
            return try! JSONSerialization.data(withJSONObject: document, options: [.sortedKeys])
        }
    }
}

/// A manifest under construction, built from real OBCG vectors.
struct ManifestBuilder {
    struct ProductSpec {
        var id: String
        var tier: UInt8
        var vectors: [String]
        var referenceTime: Date
        var generatedAt: Date
        var stalenessDeadline: Date
        var attributionText: String = "Source: Deutscher Wetterdienst (DWD)"
        var attributionURL: String = "https://creativecommons.org/licenses/by/4.0/"
        /// Overrides the bbox derived from the frames' intersection — used to model a product whose
        /// coverage is narrower or wider than the fixture objects.
        var boundsOverride: WeatherBoundingBox?
    }

    var products: [ProductSpec] = []
    /// Extra raw product JSON, for entries this build is not supposed to understand.
    var rawProducts: [[String: Any]] = []
    var version = 1
    var generatedAt = Date(timeIntervalSince1970: 1_800_000_000)

    /// Every object the manifest names, keyed by its object key, ready to hand to the stub client.
    private(set) var objects: [String: Data] = [:]

    mutating func add(_ spec: ProductSpec) throws {
        products.append(spec)
        for name in spec.vectors {
            objects[Self.key(for: name)] = try WeatherFixtures.vector(name)
        }
    }

    static func key(for vector: String) -> String {
        "wx/v1/fixtures/\(vector)"
    }

    func json() throws -> Data {
        var entries: [[String: Any]] = []
        for spec in products {
            var frames: [[String: Any]] = []
            var bounds: WeatherBoundingBox?
            for name in spec.vectors {
                let bytes = try WeatherFixtures.vector(name)
                let header = try OBCGridCodec.decodeHeader(bytes)
                let geometry: [String: Any] = [
                    "south_udeg": header.southLatitudeMicrodegrees,
                    "west_udeg": header.westLongitudeMicrodegrees,
                    "cell_lat_udeg": header.cellLatitudeStrideMicrodegrees,
                    "cell_lon_udeg": header.cellLongitudeStrideMicrodegrees,
                    "width": header.width, "height": header.height,
                    "cell_size_m": header.cellSizeMetres, "tile_edge": header.tileEdge,
                    "entries_per_page": header.entriesPerPage,
                ]
                let frameBounds = WeatherBoundingBox(
                    southMicrodegrees: Int64(header.southLatitudeMicrodegrees),
                    westMicrodegrees: Int64(header.westLongitudeMicrodegrees),
                    northMicrodegrees: header.northLatitudeMicrodegrees,
                    eastMicrodegrees: header.eastLongitudeMicrodegrees)
                bounds = bounds.map { existing in
                    WeatherBoundingBox(
                        southMicrodegrees: max(existing.southMicrodegrees, frameBounds.southMicrodegrees),
                        westMicrodegrees: max(existing.westMicrodegrees, frameBounds.westMicrodegrees),
                        northMicrodegrees: min(existing.northMicrodegrees, frameBounds.northMicrodegrees),
                        eastMicrodegrees: min(existing.eastMicrodegrees, frameBounds.eastMicrodegrees))
                } ?? frameBounds
                frames.append([
                    "offset_min": UInt32(
                        max(0, (header.validAtUnixSeconds - header.referenceTimeUnixSeconds) / 60)),
                    "valid_at": RFC3339.string(
                        from: Date(timeIntervalSince1970: TimeInterval(header.validAtUnixSeconds))),
                    "source_class": header.flags & OBCGridCodec.flagObserved != 0
                        ? "observation" : "forecast",
                    "key": Self.key(for: name),
                    "bytes": bytes.count,
                    "object_crc32": String(format: "0x%08X", header.objectCRC32),
                    "geometry": geometry,
                ])
            }
            let box = spec.boundsOverride ?? bounds!
            entries.append([
                "id": spec.id, "tier": spec.tier,
                "bbox_udeg": [
                    "south_udeg": box.southMicrodegrees, "west_udeg": box.westMicrodegrees,
                    "north_udeg": box.northMicrodegrees, "east_udeg": box.eastMicrodegrees,
                ],
                "cell": ["lat_udeg": 9_000, "lon_udeg": 14_000, "nominal_m": 1_000],
                "reference_time": RFC3339.string(from: spec.referenceTime),
                "generated_at": RFC3339.string(from: spec.generatedAt),
                "staleness_deadline": RFC3339.string(from: spec.stalenessDeadline),
                "attribution": ["text": spec.attributionText, "url": spec.attributionURL],
                "frames": frames,
            ])
        }
        entries.append(contentsOf: rawProducts)
        let document: [String: Any] = [
            "version": version,
            "generated_at": RFC3339.string(from: generatedAt),
            "products": entries,
        ]
        return try JSONSerialization.data(withJSONObject: document, options: [.sortedKeys])
    }

    /// Everything a stub client needs to serve this manifest and its objects.
    func stubObjects(entityTag: String? = "\"fixture\"") throws -> [String: StubWeatherHTTPClient.Object] {
        var served: [String: StubWeatherHTTPClient.Object] = [
            OBCWeatherServiceClient.manifestKey: StubWeatherHTTPClient.Object(
                bytes: try json(),
                headers: entityTag.map { ["ETag": $0] } ?? [:],
                entityTag: entityTag),
        ]
        for (key, bytes) in objects {
            served[key] = StubWeatherHTTPClient.Object(bytes: bytes)
        }
        return served
    }
}
