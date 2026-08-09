import Foundation
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// The OBC weather service client against fixture bytes: what it fetches, what it refuses, and what
/// it degrades to.
struct WeatherServiceClientTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    static let baseURL = URL(string: "https://wx.example.invalid/")!
    static let radarVector = "grid-multipage.obcg"
    static let radarKey = ManifestBuilder.key(for: radarVector)

    /// Cells 20...39 in both axes of the 40 x 40 radar vector: tiles 4, 5, 7 and 8, spread over
    /// three directory pages at two entries per page. The same corridor `OBCGridCodecTests` pins on
    /// the wire side.
    static let corridor = WeatherCorridor(
        bounds: WeatherBoundingBox(
            southMicrodegrees: 47_180_000, westMicrodegrees: 7_280_000,
            northMicrodegrees: 47_359_000, eastMicrodegrees: 7_559_000),
        isUndirected: true)

    static func radarManifest(
        stalenessDeadline: TimeInterval = 900, vectors: [String] = [radarVector]
    ) throws -> ManifestBuilder {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "dwd-rv", tier: 1, vectors: vectors,
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(stalenessDeadline)))
        return builder
    }

    static func client(
        _ builder: ManifestBuilder
    ) throws -> (OBCWeatherServiceClient, StubWeatherHTTPClient) {
        let http = StubWeatherHTTPClient(objects: try builder.stubObjects())
        return (OBCWeatherServiceClient(baseURL: baseURL, client: http), http)
    }

    // MARK: - The corridor read contract

    /// OBCG §7, asserted as a byte ledger: header, the covering directory pages, and only the
    /// non-dry tiles the corridor touches. Nothing else may be fetched — a client that downloaded
    /// whole frames would decode exactly the same cells and be exactly as wrong.
    @Test
    func aCorridorReadTouchesOnlyTheHeaderItsPagesAndItsNeededTiles() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        let selection = try #require(outcome.selection)
        #expect(selection.productID == "dwd-rv")

        // Independently compute the byte set the spec says is needed, from the vector itself.
        let bytes = try WeatherFixtures.vector(Self.radarVector)
        let header = try OBCGridCodec.decodeHeader(bytes)
        var needed: [Range<Int>] = [0..<OBCGridCodec.headerLength]
        var tileReads = 0
        for page in [2, 3, 4] {
            let offset = try #require(header.pageOffset(page))
            needed.append(offset..<(offset + header.pageBytes))
        }
        for tile in [4, 5, 7, 8] {
            let page = header.pageOfEntry(tile)
            let offset = try #require(header.pageOffset(page))
            let pageBytes = try #require(bytes.readBytes(at: offset, count: header.pageBytes))
            let entry = try OBCGridCodec.decodeEntry(
                page: pageBytes, indexInPage: tile - page * Int(header.entriesPerPage))
            guard !entry.isDry else { continue }  // a dry sentinel costs no read at all
            needed.append(try OBCGridCodec.payloadRange(header: header, entry: entry))
            tileReads += 1
        }
        #expect(tileReads == 3, "tile 4 is a dry sentinel in this vector")

        let fetched = http.requests(forPathSuffix: Self.radarVector).compactMap(\.byteRange)
        #expect(fetched == CorridorExtraction.coalesce(needed),
                "fetched ranges must be exactly the needed set (adjacent ranges coalesced)")
        let fetchedBytes = fetched.reduce(0) { $0 + $1.count }
        #expect(fetchedBytes < bytes.count, "a corridor never costs a whole frame")
    }

    @Test
    func theCroppedCellsMatchTheWireVectorCellForCell() async throws {
        let builder = try Self.radarManifest()
        let (service, _) = try Self.client(builder)
        let selection = try #require(
            (try await service.precipitation(for: Self.corridor, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.width == 20 && crop.height == 20)
        #expect(crop.southMicrodegrees == 47_000_000 + 20 * 9_000)
        #expect(crop.westMicrodegrees == 7_000_000 + 20 * 14_000)
        #expect(crop.validAt == Date(timeIntervalSince1970: 1_800_000_000))
        // The vector's one wet cell is its north-east corner; everything else in the corridor is
        // dry or no-data padding, exactly as the wire-side test pins it.
        #expect(crop.cells[19 * 20 + 19] == 9)
        #expect(crop.cells[0] == 0, "the dry sentinel tile decodes as dry, not as no-data")
        #expect(crop.quality.contains(.observed))
    }

    /// A corridor that reaches past the grid is answered for the part that exists and *flagged*.
    /// The alternative — quietly returning a smaller map — is how "DRY FOR 2 HOURS" gets claimed
    /// over cells nobody looked at.
    @Test
    func aCorridorReachingPastTheGridIsMarkedPartial() async throws {
        let builder = try Self.radarManifest()
        var product = builder.products[0]
        // Widen the product bbox to the whole world so selection succeeds; the frame still only
        // covers its own 40 x 40 window.
        product.boundsOverride = WeatherBoundingBox(
            southMicrodegrees: -90_000_000, westMicrodegrees: -180_000_000,
            northMicrodegrees: 90_000_000, eastMicrodegrees: 180_000_000)
        var widened = builder
        widened.products = [product]
        // The bbox-vs-frames check refuses that manifest outright, which is itself the right
        // behaviour: a product cannot claim coverage its frames do not have.
        #expect(try WeatherServiceManifest.parse(widened.json()).skippedProducts == 1)

        // The honest partial case: a corridor inside the bbox whose cells run to the grid edge.
        let edge = WeatherCorridor(
            bounds: WeatherBoundingBox(
                southMicrodegrees: 47_180_000, westMicrodegrees: 7_280_000,
                northMicrodegrees: 47_360_000, eastMicrodegrees: 7_560_000),
            isUndirected: true)
        let (service, _) = try Self.client(builder)
        let selection = try #require(
            (try await service.precipitation(for: edge, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.quality.contains(.partialCoverage))
    }

    // MARK: - Refusals

    @Test
    func aTamperedDirectoryPageIsRefusedAndTheRainMapDegradesHonestly() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        let bytes = try WeatherFixtures.vector(Self.radarVector)
        let header = try OBCGridCodec.decodeHeader(bytes)
        var corrupted = bytes
        let pageOffset = try #require(header.pageOffset(3))
        corrupted[pageOffset] ^= 0x01
        http.mutate(Self.radarKey) { $0.bytes = corrupted }

        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome == .unavailable(.framesUnavailable, outcome.diagnostics))
        #expect(outcome.selection == nil)
    }

    @Test
    func aTamperedTilePayloadIsRefused() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        var corrupted = try WeatherFixtures.vector(Self.radarVector)
        let header = try OBCGridCodec.decodeHeader(corrupted)
        // Corrupt a payload the corridor actually reads — tile 8, the wet north-east corner.
        let page = header.pageOfEntry(8)
        let pageOffset = try #require(header.pageOffset(page))
        let pageBytes = try #require(corrupted.readBytes(at: pageOffset, count: header.pageBytes))
        let entry = try OBCGridCodec.decodeEntry(
            page: pageBytes, indexInPage: 8 - page * Int(header.entriesPerPage))
        let payload = try OBCGridCodec.payloadRange(header: header, entry: entry)
        corrupted[payload.lowerBound] ^= 0xFF
        http.mutate(Self.radarKey) { $0.bytes = corrupted }
        #expect(try await service.precipitation(for: Self.corridor, now: Self.now).selection == nil)
    }

    @Test
    func aFrameWhoseHeaderContradictsTheManifestIsRefused() async throws {
        var builder = try Self.radarManifest()
        // Re-stamp the manifest's frame timestamp: the object is untouched and still verifies its
        // own CRCs, but the manifest is now claiming a frame is fresher than it is.
        var document = try JSONSerialization.jsonObject(with: builder.json()) as! [String: Any]
        var products = document["products"] as! [[String: Any]]
        var frames = products[0]["frames"] as! [[String: Any]]
        frames[0]["valid_at"] = RFC3339.string(from: Self.now.addingTimeInterval(600))
        products[0]["frames"] = frames
        document["products"] = products
        let tampered = try JSONSerialization.data(withJSONObject: document)

        var objects = try builder.stubObjects()
        objects[OBCWeatherServiceClient.manifestKey] = StubWeatherHTTPClient.Object(bytes: tampered)
        let http = StubWeatherHTTPClient(objects: objects)
        let service = OBCWeatherServiceClient(baseURL: Self.baseURL, client: http)
        #expect(try await service.precipitation(for: Self.corridor, now: Self.now).selection == nil)
        builder.products = []
    }

    @Test
    func aTileFetchThatFailsLeavesTheRestOfTheJobIntact() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        http.mutate(Self.radarKey) { $0.status = 404 }
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome.selection == nil)
        if case let .unavailable(reason, _) = outcome { #expect(reason == .framesUnavailable) }
    }

    /// A server that answers a Range request with the whole object is legal HTTP. Slicing it
    /// ourselves is the only safe reading — parsing the head of a file as though it were the middle
    /// would decode nonsense that happens to pass length checks.
    @Test
    func aServerIgnoringRangeIsHandledRatherThanMisparsed() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        http.mutate(Self.radarKey) { $0.ignoresRange = true }
        let selection = try #require(
            (try await service.precipitation(for: Self.corridor, now: Self.now)).selection)
        #expect(selection.crops.count == 1)
    }

    // MARK: - Manifest lifecycle

    @Test
    func anUnreachableManifestIsAServiceOutageNotACrash() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        http.mutate(OBCWeatherServiceClient.manifestKey) { $0.offline = true }
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome == .unavailable(.serviceUnavailable, outcome.diagnostics))
    }

    @Test
    func aMalformedManifestIsAServiceOutage() async throws {
        var objects = try Self.radarManifest().stubObjects()
        objects[OBCWeatherServiceClient.manifestKey] = StubWeatherHTTPClient.Object(
            bytes: Data("{\"version\": 99}".utf8))
        let service = OBCWeatherServiceClient(
            baseURL: Self.baseURL, client: StubWeatherHTTPClient(objects: objects))
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome == .unavailable(.serviceUnavailable, outcome.diagnostics))
    }

    @Test
    func theManifestIsRevalidatedWithItsETagAndReusedInsideItsCacheWindow() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        _ = try await service.precipitation(for: Self.corridor, now: Self.now)
        // Inside the 60 s manifest cache window: not even a conditional request.
        _ = try await service.precipitation(for: Self.corridor, now: Self.now.addingTimeInterval(30))
        #expect(http.requests(forPathSuffix: "manifest.json").count == 1)

        // Past it: one conditional request carrying the ETag, answered 304, and the parsed manifest
        // is reused rather than re-parsed from a body that never arrived.
        let outcome = try await service.precipitation(
            for: Self.corridor, now: Self.now.addingTimeInterval(120))
        let manifestRequests = http.requests(forPathSuffix: "manifest.json")
        #expect(manifestRequests.count == 2)
        #expect(manifestRequests[1].entityTag == "\"fixture\"")
        #expect(outcome.selection?.productID == "dwd-rv")
    }

    /// Frame keys are immutable, so a second corridor read of the same window costs no HTTP at all —
    /// and in particular costs no revalidation.
    @Test
    func aCachedFrameIsNeverRefetchedOrRevalidated() async throws {
        let builder = try Self.radarManifest()
        let (service, http) = try Self.client(builder)
        _ = try await service.precipitation(for: Self.corridor, now: Self.now)
        let first = http.requests(forPathSuffix: Self.radarVector).count
        #expect(first > 0)
        http.resetLedger()
        _ = try await service.precipitation(for: Self.corridor, now: Self.now.addingTimeInterval(10))
        #expect(http.requests(forPathSuffix: Self.radarVector).isEmpty)
    }

    @Test
    func concurrentCorridorJobsProduceTheSameSelectionDeterministically() async throws {
        let builder = try Self.radarManifest()
        let (service, _) = try Self.client(builder)
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
            objectKey: "wx/v1/x/f0.obcg", columnMinimum: 0, rowMinimum: 0, width: 2, height: 1)
        let crop = PrecipitationCrop(
            validAt: Self.now, southMicrodegrees: 47_000_000, westMicrodegrees: 7_000_000,
            latitudeStrideMicrodegrees: 9_000, longitudeStrideMicrodegrees: 14_000,
            width: 2, height: 1, cellSizeMetres: 1_000, quality: .observed, cells: [3, 4])
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
                objectKey: "wx/v1/x/f\(index).obcg", columnMinimum: 0, rowMinimum: 0,
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
            objectKey: "wx/v1/x/f2.obcg", columnMinimum: 0, rowMinimum: 0, width: 4, height: 2)
        #expect(await cache.crop(for: wider) == nil)
    }
}
