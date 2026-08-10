import Foundation
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// The OBC weather service client against synthesised shard bytes: what it fetches, what it refuses,
/// and what it degrades to.
///
/// The manifest here is v2 over a regional lattice (see ``ManifestV2Builder``); the cross-language
/// *document* contract is pinned separately by ``ManifestV2Tests`` against the shared fixture.
struct WeatherServiceClientTests {
    static let now = ManifestV2Builder.referenceDate
    static let baseURL = URL(string: "https://wx.example.invalid/")!

    /// Cells 8...23 in both axes — wholly inside shard (0, 0), so a single-shard read is the control
    /// for every mosaic case below.
    static let corridor = WeatherCorridor(bounds: WeatherBoundingBox(
        southMicrodegrees: 47_080_000, westMicrodegrees: 7_080_000,
        northMicrodegrees: 47_240_000, eastMicrodegrees: 7_240_000))

    /// Cells 56...71 in both axes, which straddles the 64-cell shard seam on **both** axes: one
    /// frame is then assembled out of four shard crops.
    static let seamCorridor = WeatherCorridor(bounds: WeatherBoundingBox(
        southMicrodegrees: 47_560_000, westMicrodegrees: 7_560_000,
        northMicrodegrees: 47_720_000, eastMicrodegrees: 7_720_000))

    static func client(
        _ builder: ManifestV2Builder
    ) throws -> (OBCWeatherServiceClient, StubWeatherHTTPClient) {
        let http = StubWeatherHTTPClient(objects: try builder.stubObjects())
        return (OBCWeatherServiceClient(baseURL: baseURL, client: http), http)
    }

    // MARK: - The corridor read contract

    /// OBCG §7, asserted as a byte ledger: header, the covering directory pages, and only the
    /// non-dry tiles the corridor touches. Nothing else may be fetched — a client that downloaded
    /// whole shards would decode exactly the same cells and be exactly as wrong.
    @Test
    func aCorridorReadTouchesOnlyTheHeaderItsPagesAndItsNeededTiles() async throws {
        let builder = ManifestV2Builder()
        let (service, http) = try Self.client(builder)
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        let selection = try #require(outcome.selection)
        #expect(selection.generation == "20260810T1430Z")
        #expect(selection.crops.count == 2, "both frames of the timeline")

        // Independently compute the byte set the spec says is needed, from the object itself.
        let shard = WeatherShardID(column: 0, row: 0)
        let key = builder.key(offsetMinutes: 0, shard: shard)
        let bytes = try #require(builder.objects()[key])
        let header = try OBCGridCodec.decodeHeader(bytes)
        var needed: [Range<Int>] = [0..<OBCGridCodec.headerLength]
        let pageOffset = try #require(header.pageOffset(0))
        needed.append(pageOffset..<(pageOffset + header.pageBytes))
        let pageBytes = try #require(bytes.readBytes(at: pageOffset, count: header.pageBytes))
        // Cells 8...23 touch tiles (0,0), (1,0), (0,1) and (1,1) of a 64-cell shard at edge 16 —
        // four of its sixteen, all on the first of its two directory pages.
        for tile in [0, 1, 4, 5] {
            let entry = try OBCGridCodec.decodeEntry(page: pageBytes, indexInPage: tile)
            guard !entry.isDry else { continue }
            needed.append(try OBCGridCodec.payloadRange(header: header, entry: entry))
        }

        let fetched = http.requests(forPathSuffix: "f0/s0-0.obcg").compactMap(\.byteRange)
        // Coalesced on both sides: the client necessarily reads in three steps (header, then the
        // pages it computes from it, then the payloads the pages point at), so what the ledger must
        // match is the *byte set*, not the request boundaries.
        #expect(CorridorExtraction.coalesce(fetched) == CorridorExtraction.coalesce(needed),
                "fetched ranges must be exactly the needed set")
        #expect(fetched.reduce(0) { $0 + $1.count } < bytes.count / 2,
                "a corridor never costs a whole shard")
        // The other three shards are never touched at all.
        #expect(http.requests(forPathSuffix: "s1-1.obcg").isEmpty)
    }

    @Test
    func theCroppedCellsMatchTheWrittenObjectCellForCell() async throws {
        let builder = ManifestV2Builder()
        let (service, _) = try Self.client(builder)
        let selection = try #require(
            (try await service.precipitation(for: Self.corridor, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.width == 16 && crop.height == 16)
        #expect(crop.southMicrodegrees == 47_000_000 + 8 * 10_000)
        #expect(crop.westMicrodegrees == 7_000_000 + 8 * 10_000)
        #expect(crop.latitudeStrideMicrodegrees == 10_000)
        #expect(crop.longitudeStrideMicrodegrees == 10_000)
        #expect(crop.cellSizeMetres == 1_113)
        #expect(crop.validAt == Self.now)
        for row in 0..<16 {
            for column in 0..<16 {
                #expect(crop.cells[row * 16 + column]
                    == builder.cellValue(0, 8 + column, 8 + row))
            }
        }
        #expect(crop.quality.contains(.observed))
        #expect(!crop.quality.contains(.partialCoverage))
    }

    /// **A corridor can straddle a shard seam, so one frame is assembled from up to four crops.**
    @Test
    func aCorridorAcrossAShardSeamIsAssembledFromFourShards() async throws {
        let builder = ManifestV2Builder()
        let (service, http) = try Self.client(builder)
        let selection = try #require(
            (try await service.precipitation(for: Self.seamCorridor, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.width == 16 && crop.height == 16)
        for suffix in ["f0/s0-0.obcg", "f0/s1-0.obcg", "f0/s0-1.obcg", "f0/s1-1.obcg"] {
            #expect(!http.requests(forPathSuffix: suffix).isEmpty, "\(suffix) was not read")
        }
        // The seam is invisible in the result: every cell is the lattice's own value, whichever
        // shard it came out of.
        for row in 0..<16 {
            for column in 0..<16 {
                #expect(crop.cells[row * 16 + column]
                    == builder.cellValue(0, 56 + column, 56 + row),
                    "cell (\(column), \(row)) is wrong across the seam")
            }
        }
        #expect(!crop.quality.contains(.partialCoverage))
    }

    /// **A bitmap-absent shard is DRY, and dry is painted.** Intensity 0 into the frame, not no-data
    /// and not a hole; a fully dry frame is a real all-zero frame, and that is how "no rain" renders.
    @Test
    func aBitmapAbsentShardIsPaintedDryRatherThanLeftAsNoData() async throws {
        var builder = ManifestV2Builder()
        // Shard (1, 1) is dry in the first frame — the north-east quarter of the seam corridor.
        builder.frames[0].dryShards = [WeatherShardID(column: 1, row: 1)]
        let (service, http) = try Self.client(builder)
        let selection = try #require(
            (try await service.precipitation(for: Self.seamCorridor, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)

        // No request is made for a dry shard: there is no object, and none is missing.
        #expect(http.requests(forPathSuffix: "f0/s1-1.obcg").isEmpty)
        // Its cells are intensity 0 — dry — rather than 15.
        for row in 8..<16 {
            for column in 8..<16 {
                #expect(crop.cells[row * 16 + column] == OBCPrecipitationTileCodec.dry)
            }
        }
        // The other three quarters still carry their real values, and nothing is no-data.
        #expect(crop.cells[0] == builder.cellValue(0, 56, 56))
        #expect(!crop.cells.contains(OBCPrecipitationTileCodec.noData))
        #expect(!crop.quality.contains(.partialCoverage))
    }

    /// A whole frame of dry shards is a whole frame of zeroes — a real frame in the bundle, not an
    /// absent one and not a ``NoRainMapReason``.
    @Test
    func aFullyDryFrameIsAnAllZeroFrameNotAMissingOne() async throws {
        var builder = ManifestV2Builder()
        builder.frames[0].dryShards = [
            WeatherShardID(column: 0, row: 0), WeatherShardID(column: 1, row: 0),
            WeatherShardID(column: 0, row: 1), WeatherShardID(column: 1, row: 1),
        ]
        let (service, _) = try Self.client(builder)
        let outcome = try await service.precipitation(for: Self.seamCorridor, now: Self.now)
        let selection = try #require(outcome.selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.cells.allSatisfy { $0 == OBCPrecipitationTileCodec.dry })
        #expect(outcome.diagnostics.dryShards == 4)
        // And it survives the bundle build as a real frame of zeroes.
        #expect(selection.crops.count == 2)
    }

    /// A corridor reaching past the lattice is answered for the part that exists and *flagged*. The
    /// alternative — quietly returning a smaller map — is how "DRY FOR 2 HOURS" gets claimed over
    /// cells nobody looked at.
    @Test
    func aCorridorReachingPastTheLatticeIsMarkedPartial() async throws {
        let builder = ManifestV2Builder()
        let (service, _) = try Self.client(builder)
        // The lattice ends at 48.28 / 8.28; this corridor runs past both.
        let edge = WeatherCorridor(bounds: WeatherBoundingBox(
            southMicrodegrees: 48_200_000, westMicrodegrees: 8_200_000,
            northMicrodegrees: 48_360_000, eastMicrodegrees: 8_360_000))
        let selection = try #require(
            (try await service.precipitation(for: edge, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.quality.contains(.partialCoverage))
        #expect(crop.width == 8 && crop.height == 8, "clipped to the lattice, and said so")
    }

    // MARK: - The four states that are not "no rain"

    @Test
    func aCorridorOffTheLatticeIsOutOfDomainNotADryMap() async throws {
        let (service, _) = try Self.client(ManifestV2Builder())
        let sahara = WeatherCorridor(bounds: WeatherBoundingBox(
            southMicrodegrees: 20_000_000, westMicrodegrees: 10_000_000,
            northMicrodegrees: 20_200_000, eastMicrodegrees: 10_200_000))
        let outcome = try await service.precipitation(for: sahara, now: Self.now)
        #expect(outcome == .unavailable(.outOfDomain, outcome.diagnostics))
        #expect(outcome.selection == nil)
    }

    @Test
    func aCorridorOutsideCoveredRowsIsUncoveredNotADryMap() async throws {
        var builder = ManifestV2Builder()
        // The baker reaches only the southern half of this lattice.
        builder.coveredRows = 0..<64
        let (service, http) = try Self.client(builder)
        let north = WeatherCorridor(bounds: WeatherBoundingBox(
            southMicrodegrees: 47_800_000, westMicrodegrees: 7_080_000,
            northMicrodegrees: 47_960_000, eastMicrodegrees: 7_240_000))
        let outcome = try await service.precipitation(for: north, now: Self.now)
        #expect(outcome == .unavailable(.uncovered, outcome.diagnostics))
        #expect(http.requests(forPathSuffix: ".obcg").isEmpty,
                "objects exist there and are all intensity 15; fetching them buys nothing")
    }

    @Test
    func anExpiredGenerationIsNoWeatherNotADryMap() async throws {
        let builder = ManifestV2Builder()
        let (service, http) = try Self.client(builder)
        let outcome = try await service.precipitation(
            for: Self.corridor, now: builder.staleAfter.addingTimeInterval(1))
        #expect(outcome == .unavailable(
            .expired(staleAfter: builder.staleAfter), outcome.diagnostics))
        #expect(http.requests(forPathSuffix: ".obcg").isEmpty)
    }

    @Test
    func anUnreachableManifestIsAServiceOutageNotACrash() async throws {
        let (service, http) = try Self.client(ManifestV2Builder())
        http.mutate(OBCWeatherServiceClient.manifestKey) { $0.offline = true }
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome == .unavailable(.serviceUnavailable, outcome.diagnostics))
    }

    @Test
    func aMalformedManifestIsAServiceOutage() async throws {
        var objects = try ManifestV2Builder().stubObjects()
        objects[OBCWeatherServiceClient.manifestKey] = StubWeatherHTTPClient.Object(
            bytes: Data("{\"version\": 99}".utf8))
        let service = OBCWeatherServiceClient(
            baseURL: Self.baseURL, client: StubWeatherHTTPClient(objects: objects))
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome == .unavailable(.serviceUnavailable, outcome.diagnostics))
    }

    // MARK: - Refusals

    /// **A present shard that fails is an error, never dry.** Every way an object can betray the
    /// manifest fails its frame; losing every frame is `framesUnavailable`, which is a state the
    /// rider is shown — and is not a map of zeroes.
    @Test
    func aPresentShardThatFailsIsAnErrorRatherThanDry() async throws {
        let builder = ManifestV2Builder()
        let key = builder.key(offsetMinutes: 0, shard: WeatherShardID(column: 0, row: 0))
        let secondKey = builder.key(offsetMinutes: 15, shard: WeatherShardID(column: 0, row: 0))
        let bytes = try #require(builder.objects()[key])
        let header = try OBCGridCodec.decodeHeader(bytes)

        var corruptions: [(String, (inout StubWeatherHTTPClient.Object) -> Void)] = [
            ("a 404", { $0.status = 404 }),
        ]
        let pageOffset = try #require(header.pageOffset(0))
        var tamperedPage = bytes
        tamperedPage[pageOffset] ^= 0x01
        corruptions.append(("a tampered directory page", { $0.bytes = tamperedPage }))

        let pageBytes = try #require(bytes.readBytes(at: pageOffset, count: header.pageBytes))
        // Tile 5 is one the corridor actually reads; corrupting a tile it skips would prove nothing.
        let entry = try OBCGridCodec.decodeEntry(page: pageBytes, indexInPage: 5)
        var tamperedPayload = bytes
        tamperedPayload[try OBCGridCodec.payloadRange(header: header, entry: entry).lowerBound]
            ^= 0xFF
        corruptions.append(("a tampered tile payload", { $0.bytes = tamperedPayload }))

        for (why, corrupt) in corruptions {
            let (service, http) = try Self.client(builder)
            http.mutate(key, corrupt)
            http.mutate(secondKey, corrupt)
            let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
            #expect(outcome.selection == nil, "\(why) must not produce a rain map")
            if case let .unavailable(reason, _) = outcome {
                #expect(reason == .framesUnavailable, "\(why)")
            }
        }
    }

    @Test
    func aShardWhoseHeaderContradictsTheManifestIsRefused() async throws {
        let builder = ManifestV2Builder()
        var objects = try builder.stubObjects()
        // Re-stamp the manifest's frame timestamp: the objects are untouched and still verify their
        // own CRCs, but the manifest now claims a frame is fresher than it is.
        var document = try JSONSerialization.jsonObject(with: builder.json()) as! [String: Any]
        var frames = document["frames"] as! [[String: Any]]
        frames[0]["valid_at"] = RFC3339.string(from: Self.now.addingTimeInterval(600))
        frames[1]["valid_at"] = RFC3339.string(from: Self.now.addingTimeInterval(1_500))
        document["frames"] = frames
        objects[OBCWeatherServiceClient.manifestKey] = StubWeatherHTTPClient.Object(
            bytes: try JSONSerialization.data(withJSONObject: document))
        let service = OBCWeatherServiceClient(
            baseURL: Self.baseURL, client: StubWeatherHTTPClient(objects: objects))
        #expect(try await service.precipitation(
            for: Self.corridor, now: Self.now).selection == nil)
    }

    /// A server that answers a Range request with the whole object is legal HTTP. Slicing it
    /// ourselves is the only safe reading — parsing the head of a file as though it were the middle
    /// would decode nonsense that happens to pass length checks.
    @Test
    func aServerIgnoringRangeIsHandledRatherThanMisparsed() async throws {
        let builder = ManifestV2Builder()
        let (service, http) = try Self.client(builder)
        for key in builder.objects().keys { http.mutate(key) { $0.ignoresRange = true } }
        let selection = try #require(
            (try await service.precipitation(for: Self.corridor, now: Self.now)).selection)
        #expect(selection.crops.count == 2)
    }

    // MARK: - Manifest lifecycle

    @Test
    func theManifestIsRevalidatedWithItsETagAndReusedInsideItsOwnStatedWindow() async throws {
        let builder = ManifestV2Builder()
        let (service, http) = try Self.client(builder)
        _ = try await service.precipitation(for: Self.corridor, now: Self.now)
        // Inside the document's own `manifest_max_age_s`: not even a conditional request.
        _ = try await service.precipitation(
            for: Self.corridor, now: Self.now.addingTimeInterval(30))
        #expect(http.requests(forPathSuffix: "manifest.json").count == 1)

        // Past it: one conditional request carrying the ETag, answered 304, and the parsed manifest
        // is reused rather than re-parsed from a body that never arrived.
        let outcome = try await service.precipitation(
            for: Self.corridor, now: Self.now.addingTimeInterval(120))
        let manifestRequests = http.requests(forPathSuffix: "manifest.json")
        #expect(manifestRequests.count == 2)
        #expect(manifestRequests[1].entityTag == "\"fixture\"")
        #expect(outcome.selection?.generation == "20260810T1430Z")
    }

    /// Shard keys are immutable — the generation is a key segment — so a second corridor read of the
    /// same window costs no HTTP at all, and in particular costs no revalidation.
    @Test
    func aCachedShardIsNeverRefetchedOrRevalidated() async throws {
        let (service, http) = try Self.client(ManifestV2Builder())
        _ = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(!http.requests(forPathSuffix: "s0-0.obcg").isEmpty)
        http.resetLedger()
        _ = try await service.precipitation(
            for: Self.corridor, now: Self.now.addingTimeInterval(10))
        #expect(http.requests(forPathSuffix: ".obcg").isEmpty)
    }

    @Test
    func concurrentCorridorJobsProduceTheSameCropsDeterministically() async throws {
        let (service, _) = try Self.client(ManifestV2Builder())
        async let first = service.precipitation(for: Self.corridor, now: Self.now)
        async let second = service.precipitation(for: Self.corridor, now: Self.now)
        let outcomes = try await [first, second]
        #expect(outcomes[0].selection?.crops == outcomes[1].selection?.crops)
    }

    // MARK: - Cache behaviour

    @Test
    func aCorruptCacheEntryIsACleanMiss() async throws {
        let directory = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("obcwx-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let cache = FileWeatherFrameCache(directory: directory)
        let key = WeatherFrameCacheKey(
            objectKey: "wx/v2/g/f0/s0-0.obcg", columnMinimum: 0, rowMinimum: 0,
            width: 2, height: 1)
        let crop = PrecipitationCrop(
            validAt: Self.now, southMicrodegrees: 47_000_000, westMicrodegrees: 7_000_000,
            latitudeStrideMicrodegrees: 10_000, longitudeStrideMicrodegrees: 10_000,
            width: 2, height: 1, cellSizeMetres: 1_113, quality: .observed, cells: [3, 4])
        await cache.store(crop, for: key)
        #expect(await cache.crop(for: key) == crop)

        // Flip a byte of every cached file: a corrupt entry must read as absent, never as data.
        for file in try FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil) {
            var bytes = try Data(contentsOf: file)
            bytes[bytes.count / 2] ^= 0xFF
            try bytes.write(to: file)
        }
        #expect(await cache.crop(for: key) == nil)
    }

    @Test
    func theInMemoryCacheIsBoundedAndKeyedByWindow() async throws {
        let cache = InMemoryWeatherFrameCache(capacity: 2)
        func key(_ index: Int) -> WeatherFrameCacheKey {
            WeatherFrameCacheKey(
                objectKey: "wx/v2/g/f\(index)/s0-0.obcg", columnMinimum: 0, rowMinimum: 0,
                width: 1, height: 2)
        }
        let crop = PrecipitationCrop(
            validAt: Self.now, southMicrodegrees: 0, westMicrodegrees: 0,
            latitudeStrideMicrodegrees: 1, longitudeStrideMicrodegrees: 1, width: 1, height: 2,
            cellSizeMetres: 1, quality: .forecast, cells: [0, 0])
        for index in 0..<3 { await cache.store(crop, for: key(index)) }
        #expect(await cache.crop(for: key(0)) == nil, "oldest entry evicted")
        #expect(await cache.crop(for: key(2)) != nil)
        // A different window over the same object is a different question.
        let wider = WeatherFrameCacheKey(
            objectKey: "wx/v2/g/f2/s0-0.obcg", columnMinimum: 0, rowMinimum: 0,
            width: 4, height: 2)
        #expect(await cache.crop(for: wider) == nil)
    }

    // MARK: - The writer this suite depends on

    /// The synthesised shards are real OBCG objects by the codec's own acceptance check. Without
    /// this, every assertion above could be passing against bytes only the writer and the reader
    /// agree on.
    @Test
    func everySynthesisedShardPassesFullObjectValidation() throws {
        var builder = ManifestV2Builder()
        builder.frames[0].dryShards = [WeatherShardID(column: 1, row: 1)]
        let objects = builder.objects()
        #expect(objects.count == 7, "four shards at f0 minus one dry, plus four at f15")
        for (key, bytes) in objects {
            let header = try OBCGridCodec.validate(bytes)
            #expect(header.width == 64 && header.height == 64, "\(key)")
            #expect(header.cellLatitudeStrideMicrodegrees == 10_000, "\(key)")
            #expect(header.cellLongitudeStrideMicrodegrees == 10_000, "\(key)")
        }
    }
}
