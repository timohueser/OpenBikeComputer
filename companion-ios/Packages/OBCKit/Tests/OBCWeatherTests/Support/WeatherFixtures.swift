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

/// A manifest-v2 document and the shard objects it names, built together so neither can drift.
///
/// A **regional** lattice rather than the production 36,000 x 18,000 one: the cross-language contract
/// is pinned by `specs/vectors/wx-manifest-v2.json` (see `ManifestV2Tests`), and this builder's job is
/// the other half — driving the *fetch path* over real bytes, which means objects small enough to
/// synthesise. 128 x 128 cells at 0.01°, cut into four 64 x 64 shards of sixteen tiles over two
/// directory pages, is the smallest lattice that has a shard seam a corridor can straddle **and**
/// shards big enough that a corridor read is visibly a fraction of the object.
struct ManifestV2Builder {
    struct FrameSpec {
        var offsetMinutes: UInt32
        var validAt: Date
        var observed: Bool
        /// Shards the baker measured as dry everywhere, so no object is published for them. They are
        /// absent from both the bitmap and `shards[]` — which is exactly what makes them dry rather
        /// than missing.
        var dryShards: Set<WeatherShardID> = []
    }

    static let referenceDate = Date(timeIntervalSince1970: 1_800_000_000)

    var latticeSouthMicrodegrees: Int32 = 47_000_000
    var latticeWestMicrodegrees: Int32 = 7_000_000
    var cellMicrodegrees: UInt32 = 10_000
    var width: UInt32 = 128
    var height: UInt32 = 128
    var shardWidth: UInt32 = 64
    var shardHeight: UInt32 = 64
    var tileEdge: UInt16 = 16
    var entriesPerPage: UInt16 = 8
    var cellSizeMetres: UInt16 = 1_113
    var coveredRows: Range<UInt32> = 0..<128
    var keyPrefix = "wx/v2"
    var generation = "20260810T1430Z"
    var previousGenerations = ["20260810T1415Z"]
    var version = 2
    var generatedAt = referenceDate
    var referenceTime = referenceDate.addingTimeInterval(-300)
    var staleAfter = referenceDate.addingTimeInterval(900)
    var nextGenerationExpectedAt = referenceDate.addingTimeInterval(900)
    var manifestMaximumAgeSeconds = 60
    var frames: [FrameSpec] = [
        FrameSpec(offsetMinutes: 0, validAt: referenceDate, observed: true),
        FrameSpec(
            offsetMinutes: 15, validAt: referenceDate.addingTimeInterval(900), observed: false),
    ]
    /// The intensity at one lattice cell of one frame. Deterministic, and deliberately never 15:
    /// a no-data cell would raise partial coverage everywhere and hide the flag's real meaning.
    var cellValue: (UInt32, Int, Int) -> UInt8 = { offsetMinutes, column, row in
        UInt8((column + row + Int(offsetMinutes) / 15) % 13)
    }

    var shardColumns: UInt32 { (width + shardWidth - 1) / shardWidth }
    var shardRows: UInt32 { (height + shardHeight - 1) / shardHeight }

    func key(offsetMinutes: UInt32, shard: WeatherShardID) -> String {
        "\(keyPrefix)/\(generation)/f\(offsetMinutes)/s\(shard.column)-\(shard.row).obcg"
    }

    private func object(frame: FrameSpec, shard: WeatherShardID) -> Data {
        let columnOrigin = Int(shard.column * shardWidth)
        let rowOrigin = Int(shard.row * shardHeight)
        let shardCellColumns = Int(min(shardWidth, width - shard.column * shardWidth))
        let shardCellRows = Int(min(shardHeight, height - shard.row * shardHeight))
        var cells: [UInt8] = []
        cells.reserveCapacity(shardCellColumns * shardCellRows)
        for row in 0..<shardCellRows {
            for column in 0..<shardCellColumns {
                cells.append(cellValue(frame.offsetMinutes, columnOrigin + column, rowOrigin + row))
            }
        }
        return OBCGridWriter.encode(OBCGridWriter.Spec(
            southMicrodegrees: latticeSouthMicrodegrees
                + Int32(rowOrigin) * Int32(cellMicrodegrees),
            westMicrodegrees: latticeWestMicrodegrees
                + Int32(columnOrigin) * Int32(cellMicrodegrees),
            cellMicrodegrees: cellMicrodegrees, width: UInt32(shardCellColumns),
            height: UInt32(shardCellRows), tileEdge: tileEdge, entriesPerPage: entriesPerPage,
            cellSizeMetres: cellSizeMetres, validAt: frame.validAt,
            referenceTime: referenceTime, observed: frame.observed, cells: cells))
    }

    /// Every object this manifest names, keyed the way the client composes the key.
    func objects() -> [String: Data] {
        var published: [String: Data] = [:]
        for frame in frames {
            for row in 0..<shardRows {
                for column in 0..<shardColumns {
                    let shard = WeatherShardID(column: column, row: row)
                    guard !frame.dryShards.contains(shard) else { continue }
                    published[key(offsetMinutes: frame.offsetMinutes, shard: shard)] =
                        object(frame: frame, shard: shard)
                }
            }
        }
        return published
    }

    func json() throws -> Data {
        let published = objects()
        var frameEntries: [[String: Any]] = []
        for frame in frames {
            var presence = [UInt8](repeating: 0, count: Int((shardColumns * shardRows + 7) / 8))
            var shards: [[String: Any]] = []
            for row in 0..<shardRows {
                for column in 0..<shardColumns {
                    let shard = WeatherShardID(column: column, row: row)
                    guard let bytes = published[key(offsetMinutes: frame.offsetMinutes, shard: shard)]
                    else { continue }
                    let bit = row * shardColumns + column
                    presence[Int(bit / 8)] |= UInt8(1 << (bit % 8))
                    let header = try OBCGridCodec.decodeHeader(bytes)
                    shards.append([
                        "col": column, "row": row, "bytes": bytes.count,
                        "object_crc32": String(format: "0x%08X", header.objectCRC32),
                        "observed": frame.observed,
                    ])
                }
            }
            frameEntries.append([
                "offset_min": frame.offsetMinutes,
                "valid_at": RFC3339.string(from: frame.validAt),
                "present": presence.map { String(format: "%02x", $0) }.joined(),
                "shards": shards,
            ])
        }
        let document: [String: Any] = [
            "version": version,
            "generation": generation,
            "generated_at": RFC3339.string(from: generatedAt),
            "reference_time": RFC3339.string(from: referenceTime),
            "key_prefix": keyPrefix,
            "previous_generations": previousGenerations,
            "lattice": [
                "south_lat_udeg": latticeSouthMicrodegrees,
                "west_lon_udeg": latticeWestMicrodegrees,
                "cell_udeg": cellMicrodegrees, "width": width, "height": height,
                "shard_width": shardWidth, "shard_height": shardHeight,
                "shard_cols": shardColumns, "shard_rows": shardRows,
                "tile_edge": tileEdge, "entries_per_page": entriesPerPage,
                "cell_size_m": cellSizeMetres,
                "covered_rows": ["start": coveredRows.lowerBound, "end": coveredRows.upperBound],
            ],
            "cadence": [
                "frame_step_min": 15, "frames": frames.count, "max_source_skew_s": 1_800,
            ],
            "freshness": [
                "manifest_max_age_s": manifestMaximumAgeSeconds,
                "next_generation_expected_at": RFC3339.string(from: nextGenerationExpectedAt),
                "stale_after": RFC3339.string(from: staleAfter),
            ],
            "attribution": [
                ["source_id": "dwd-rv", "text": "Source: Deutscher Wetterdienst (DWD)",
                 "url": "https://creativecommons.org/licenses/by/4.0/"],
            ],
            "frames": frameEntries,
        ]
        return try JSONSerialization.data(withJSONObject: document, options: [.sortedKeys])
    }

    /// Everything a stub client needs to serve this manifest and its shards.
    func stubObjects(
        entityTag: String? = "\"fixture\""
    ) throws -> [String: StubWeatherHTTPClient.Object] {
        var served: [String: StubWeatherHTTPClient.Object] = [
            OBCWeatherServiceClient.manifestKey: StubWeatherHTTPClient.Object(
                bytes: try json(),
                headers: entityTag.map { ["ETag": $0] } ?? [:],
                entityTag: entityTag),
        ]
        for (key, bytes) in objects() {
            served[key] = StubWeatherHTTPClient.Object(bytes: bytes)
        }
        return served
    }
}
