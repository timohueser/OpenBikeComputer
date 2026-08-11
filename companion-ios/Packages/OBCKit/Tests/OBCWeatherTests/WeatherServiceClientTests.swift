import Foundation
import OBCDomain
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

    /// Every shard of the default builder's 2 x 2 lattice.
    static let everyShard: Set<WeatherShardID> = [
        WeatherShardID(column: 0, row: 0), WeatherShardID(column: 1, row: 0),
        WeatherShardID(column: 0, row: 1), WeatherShardID(column: 1, row: 1),
    ]

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
        builder.frames[0].dryShards = Self.everyShard
        let (service, _) = try Self.client(builder)
        let outcome = try await service.precipitation(for: Self.seamCorridor, now: Self.now)
        let selection = try #require(outcome.selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.cells.allSatisfy { $0 == OBCPrecipitationTileCodec.dry })
        #expect(outcome.diagnostics.dryShards == 4)
        // And it survives the bundle build as a real frame of zeroes.
        #expect(selection.crops.count == 2)
    }

    /// **The frame's quality flag follows its place in the timeline, not its content and not the
    /// per-shard `observed` bits.**
    ///
    /// OBCW carries one flag for a mosaic that is radar over the rider and model fill across the
    /// seam, so no content rule can be true of all of it — and a content rule made an all-dry
    /// frame's flag depend on whether the baker happened to publish an object, which is how the two
    /// clients came to disagree about the commonest scene there is (a dry day). The rule is: offset
    /// 0 within the dataset's own `max_source_skew_s` of now is the analysis; every forward frame is
    /// a forecast. An all-dry radar scan is still an observation.
    @Test
    func aFullyDryFrameIsObservedAtOffsetZeroAndAForecastAhead() async throws {
        var builder = ManifestV2Builder()
        builder.frames[0].dryShards = Self.everyShard
        builder.frames[1].dryShards = Self.everyShard
        let (service, http) = try Self.client(builder)
        let selection = try #require(
            (try await service.precipitation(for: Self.seamCorridor, now: Self.now)).selection)
        #expect(selection.crops.count == 2, "two dry frames are two real frames")
        #expect(http.requests(forPathSuffix: ".obcg").isEmpty, "nothing was published to fetch")
        #expect(selection.crops.allSatisfy { crop in
            crop.cells.allSatisfy { $0 == OBCPrecipitationTileCodec.dry }
        })
        #expect(selection.crops[0].quality.contains(.observed),
                "an all-dry radar scan IS an observation")
        #expect(!selection.crops[0].quality.contains(.forecast))
        #expect(selection.crops[1].quality.contains(.forecast),
                "an all-dry forecast frame is NOT")
        #expect(!selection.crops[1].quality.contains(.observed))
    }

    /// The other half of the same rule: the per-shard bits are a **diagnostics counter**, and a
    /// forward frame whose every shard claims radar is still a forecast.
    @Test
    func thePerShardObservedBitsAreACounterAndNeverTheFrameFlag() async throws {
        var builder = ManifestV2Builder()
        // Every shard of the forward frame says a radar painted it...
        builder.frames[1].observed = true
        let (service, _) = try Self.client(builder)
        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        let selection = try #require(outcome.selection)
        #expect(selection.crops[0].quality.contains(.observed))
        // ...and it is still a forecast, because offset 15 is fifteen minutes ahead of now.
        #expect(selection.crops[1].quality.contains(.forecast))
        #expect(!selection.crops[1].quality.contains(.observed))
        #expect(outcome.diagnostics.observedShards == 2,
                "the bits survive as evidence, one per shard, and nothing branches on them")
    }

    /// And offset 0 is only the analysis *while it is one*: past the dataset's own
    /// `max_source_skew_s` the same frame is no longer an observation of now.
    @Test
    func anAgedOffsetZeroFrameStopsClaimingToBeAnObservation() async throws {
        var builder = ManifestV2Builder()
        builder.staleAfter = ManifestV2Builder.referenceDate.addingTimeInterval(4 * 3_600)
        builder.nextGenerationExpectedAt = builder.staleAfter
        let (service, _) = try Self.client(builder)

        // 1,800 s is the fixture's stated skew: at the edge the frame is still the analysis.
        let atTheEdge = try #require((try await service.precipitation(
            for: Self.corridor, now: Self.now.addingTimeInterval(1_800))).selection)
        #expect(atTheEdge.crops[0].quality.contains(.observed))
        // A second past it, it is a forecast — a stale scan, honestly labelled.
        let past = try #require((try await service.precipitation(
            for: Self.corridor, now: Self.now.addingTimeInterval(1_801))).selection)
        #expect(past.crops[0].quality.contains(.forecast))
        #expect(!past.crops[0].quality.contains(.observed))
    }

    /// **A lattice that is not a whole number of shards wide, fetched.**
    ///
    /// `edgeShardsAreShortAndTheirGeometryIsDerived` tests the arithmetic; this drives the bytes.
    /// The derived narrow geometry is what the fetched header is checked against, so if the client
    /// rounded an edge shard up to a full square it would refuse every edge shard on the planet —
    /// and every other fetch test here uses a lattice that divides exactly, so nothing would say so.
    @Test
    func aShortEdgeShardIsFetchedAndItsNarrowGeometryAccepted() async throws {
        var builder = ManifestV2Builder()
        builder.width = 70   // 64 + 6: the last shard column is six cells wide
        builder.height = 70
        builder.coveredRows = 0..<70
        let (service, http) = try Self.client(builder)

        let edgeShard = WeatherShardID(column: 1, row: 0)
        #expect(builder.shardColumns == 2 && builder.shardRows == 2)
        let object = try #require(builder.objects()[
            builder.key(offsetMinutes: 0, shard: edgeShard)])
        #expect(try OBCGridCodec.validate(object).width == 6, "the published object is short")

        // Cells 60...69 in longitude straddle the seam onto the six-cell column.
        let corridor = WeatherCorridor(bounds: WeatherBoundingBox(
            southMicrodegrees: 47_000_000 + 60 * 10_000,
            westMicrodegrees: 7_000_000 + 60 * 10_000,
            northMicrodegrees: 47_000_000 + 70 * 10_000,
            eastMicrodegrees: 7_000_000 + 70 * 10_000))
        let selection = try #require(
            (try await service.precipitation(for: corridor, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(!http.requests(forPathSuffix: "f0/s1-0.obcg").isEmpty,
                "the short shard was read, not refused for disagreeing with a width nobody publishes")
        #expect(!crop.cells.contains(OBCPrecipitationTileCodec.noData),
                "every cell of the window exists on the lattice")
        for row in 0..<crop.height {
            for column in 0..<crop.width {
                #expect(crop.cells[row * crop.width + column]
                    == builder.cellValue(0, 60 + column, 60 + row))
            }
        }
    }

    /// A corridor reaching past the lattice is answered **short**, and short is not partial.
    ///
    /// This test used to assert the opposite, and the hazard its old comment named is real — quietly
    /// returning a smaller map is how "DRY FOR 2 HOURS" could get claimed over cells nobody looked
    /// at. What changed is where that hazard is answered. `OBCW_Spec.md` §5.1 defines partial
    /// coverage as some **in-bounds** cell being unavailable, and every cell of a clamped window is
    /// known, so raising the flag here tells the device that cells it can see are unknown. Nor is
    /// the flag what protects the dry claim: `obc-app`'s `rain_outlook` never reads it — the claim
    /// is refused by no-data *samples* and by `pos_in_grid`/`current_pos_in_grid`, which are
    /// geometric, so a ride leaving the stated window cannot be claimed dry whatever this flag says.
    /// Rust decides it the same way, and the two now pin each other.
    @Test
    func aCorridorReachingPastTheLatticeIsAnsweredShortRatherThanFlagged() async throws {
        let builder = ManifestV2Builder()
        let (service, _) = try Self.client(builder)
        // The lattice ends at 48.28 / 8.28; this corridor runs past both.
        let edge = WeatherCorridor(bounds: WeatherBoundingBox(
            southMicrodegrees: 48_200_000, westMicrodegrees: 8_200_000,
            northMicrodegrees: 48_360_000, eastMicrodegrees: 8_360_000))
        let selection = try #require(
            (try await service.precipitation(for: edge, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.width == 8 && crop.height == 8, "clamped to the lattice — the window really is short")
        #expect(!crop.cells.contains(OBCPrecipitationTileCodec.noData), "…and every cell of it is known")
        #expect(!crop.quality.contains(.partialCoverage), "so nothing in bounds is unavailable")
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

    /// **Frames outside the window are not fetched, and "outside the window" is not "failed".**
    ///
    /// Two hours ahead is the question the rain map answers, and an observation older than six hours
    /// would be a lie told with a true timestamp; both are properties of the timeline. When *every*
    /// published frame is outside it and nothing failed, the rider is owed a sentence about time
    /// rather than one about a download — `framesUnavailable` there would blame a service that
    /// answered perfectly.
    @Test
    func everyFrameOutsideTheWindowIsItsOwnReasonRatherThanAFailure() async throws {
        var builder = ManifestV2Builder()
        let base = ManifestV2Builder.referenceDate
        builder.frames = [
            // Seven hours old: past `maximumObservationAge`.
            ManifestV2Builder.FrameSpec(
                offsetMinutes: 0, validAt: base.addingTimeInterval(-7 * 3_600), observed: true),
            // Three hours ahead: past `horizon`.
            ManifestV2Builder.FrameSpec(
                offsetMinutes: 15, validAt: base.addingTimeInterval(3 * 3_600), observed: false),
        ]
        // The oldest frame's validity has to sit at or after the generation's upstream run
        // (OBCG §1), so the run moves back with it.
        builder.referenceTime = base.addingTimeInterval(-8 * 3_600)
        builder.staleAfter = base.addingTimeInterval(4 * 3_600)
        builder.nextGenerationExpectedAt = builder.staleAfter
        let (service, http) = try Self.client(builder)

        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        #expect(outcome.selection == nil)
        if case let .unavailable(reason, _) = outcome {
            #expect(reason == .outsideWindow, "nothing failed; the data is about a different time")
        }
        #expect(outcome.diagnostics.framesOutsideWindow == 2)
        #expect(outcome.diagnostics.failedShards == 0)
        #expect(http.requests(forPathSuffix: ".obcg").isEmpty,
                "a frame nobody can use is not worth a Range read")
    }

    /// And the filter is per frame, not per timeline: the usable half of a straddling generation
    /// still ships.
    @Test
    func aFrameInsideTheWindowSurvivesOneOutsideIt() async throws {
        var builder = ManifestV2Builder()
        let base = ManifestV2Builder.referenceDate
        builder.frames[1].validAt = base.addingTimeInterval(3 * 3_600)
        builder.staleAfter = base.addingTimeInterval(4 * 3_600)
        builder.nextGenerationExpectedAt = builder.staleAfter
        let (service, http) = try Self.client(builder)

        let outcome = try await service.precipitation(for: Self.corridor, now: Self.now)
        let selection = try #require(outcome.selection)
        #expect(selection.crops.count == 1, "only the frame inside the window")
        #expect(outcome.diagnostics.framesOutsideWindow == 1)
        #expect(http.requests(forPathSuffix: "f15/s0-0.obcg").isEmpty)
        #expect(!http.requests(forPathSuffix: "f0/s0-0.obcg").isEmpty)
    }

    /// **A whole fetch at a date-line-clamped corridor.**
    ///
    /// The corridor is cut at ±180° rather than wrapped (`OBCW_Spec.md` §1 forbids a wrapped bundle
    /// window), and the ordinary 47 °N path never exercises that: the Range reads, the corridor
    /// extraction and the window arithmetic all run against a clamped bbox here, on a lattice whose
    /// east edge *is* the date line.
    @Test
    func aCorridorClampedAtTheDateLineIsFetchedEndToEnd() async throws {
        var builder = ManifestV2Builder()
        // 128 cells of 0.01° ending exactly on 180°.
        builder.latticeWestMicrodegrees = 180_000_000 - 128 * 10_000
        let (service, http) = try Self.client(builder)
        let rider = Coordinate(latitude: 47.6, longitude: 179.9)
        let corridor = try #require(WeatherCorridor.around(
            WeatherRequest(requestID: 1, position: rider, fixTime: Self.now)) as WeatherCorridor?)
        #expect(corridor.bounds.eastMicrodegrees == 180_000_000, "the disc is cut, not wrapped")
        #expect(corridor.bounds.westMicrodegrees < corridor.bounds.eastMicrodegrees)

        let outcome = try await service.precipitation(for: corridor, now: Self.now)
        let selection: PrecipitationSelection = try #require(outcome.selection)
        let crop: PrecipitationCrop = try #require(selection.crops.first)
        #expect(crop.bounds.eastMicrodegrees <= 180_000_000, "no window may cross the antimeridian")
        #expect(!http.requests(forPathSuffix: ".obcg").isEmpty, "real Range reads were issued")
        // The east edge cells are the lattice's own, read out of the shard that owns them.
        let lastColumn = crop.width - 1
        let column = Int((crop.westMicrodegrees - Int64(builder.latticeWestMicrodegrees)) / 10_000)
            + lastColumn
        let row = Int((crop.southMicrodegrees - Int64(builder.latticeSouthMicrodegrees)) / 10_000)
        #expect(crop.cells[lastColumn] == builder.cellValue(0, column, row))
        // The disc reaches past the lattice, so the answer is **short** — and short is not partial.
        // Every cell of the window it does state has data, and §5.1's flag is about in-bounds cells
        // being unavailable. Flagging here would tell the device that cells it can see are unknown.
        #expect(!crop.quality.contains(.partialCoverage), "a clamped window is smaller, not less certain")
        #expect(!crop.cells.contains(OBCPrecipitationTileCodec.noData))
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

    /// **A present shard that fails is a hole in its frame, and the frame still ships.**
    ///
    /// This is the granularity Rust's `read_plan` settled on and the phone now matches: dropping the
    /// frame would throw away the three shards that arrived to punish the one that did not, and a
    /// shorter timeline is a worse answer than a frame with a stated hole. The hole is no-data,
    /// which is distinguishable from dry at every layer below, so it can never make an outage look
    /// rain-free — and it raises partial coverage, which is the flag that says so.
    @Test
    func oneFailedShardIsAHoleInItsFrameRatherThanTheLossOfTheFrame() async throws {
        let builder = ManifestV2Builder()
        let (service, http) = try Self.client(builder)
        // The north-east quarter of the seam corridor 404s at f0 only; f15 is untouched.
        http.mutate(builder.key(offsetMinutes: 0, shard: WeatherShardID(column: 1, row: 1))) {
            $0.status = 404
        }
        let outcome = try await service.precipitation(for: Self.seamCorridor, now: Self.now)
        let selection = try #require(outcome.selection)
        #expect(selection.crops.count == 2, "the timeline keeps its length")
        #expect(outcome.diagnostics.failedShards == 1)
        #expect(outcome.diagnostics.dryShards == 0, "nothing here was measured dry")

        let holed = selection.crops[0]
        // The three shards that answered are real, cell for cell...
        #expect(holed.cells[0] == builder.cellValue(0, 56, 56))
        #expect(holed.cells[7 * 16 + 7] == builder.cellValue(0, 63, 63))
        // ...and the quarter that failed is no-data end to end. Not one cell of it is dry.
        for row in 8..<16 {
            for column in 8..<16 {
                #expect(holed.cells[row * 16 + column] == OBCPrecipitationTileCodec.noData,
                        "a failed shard's cell (\(column), \(row)) must be no-data, never dry")
            }
        }
        #expect(holed.quality.contains(.partialCoverage), "the hole is declared")
        // The untouched frame is whole.
        #expect(!selection.crops[1].cells.contains(OBCPrecipitationTileCodec.noData))
        #expect(!selection.crops[1].quality.contains(.partialCoverage))
    }

    /// **A present shard that fails is an error, never dry.** Every way an object can betray the
    /// manifest costs its shard; losing every shard of every frame is `framesUnavailable`, which is
    /// a state the rider is shown — and is not a map of zeroes.
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
            // The corridor is one shard wide, so "every shard of every frame failed" is two
            // failures — and none of them was counted, or painted, as dry.
            #expect(outcome.diagnostics.failedShards == 2, "\(why)")
            #expect(outcome.diagnostics.dryShards == 0, "\(why)")
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

    /// **The writer is anchored to Rust-produced bytes, not just to our own reader.**
    ///
    /// `everySynthesisedShardPassesFullObjectValidation` proves the objects are *acceptable*, but
    /// acceptance is a same-language check: a writer and a decoder sharing one misunderstanding
    /// would pass it together. So the writer is pointed at a committed vector — bytes
    /// `host/obc-wx-bake`'s encoder produced — given nothing but that object's own geometry and its
    /// decoded cells, and the result must be the object back, byte for byte. Every canonical choice
    /// is in scope: the header field layout, the directory page's zero-padded entries and trailing
    /// CRC, tight payload packing in tile order, the raw4/RLE4 threshold, the dry sentinel, and both
    /// object CRCs with their stamping order.
    ///
    /// The four vectors are the deflate-free ones, and between them they cover every canonical
    /// choice this writer makes: a raw4 tile, two RLE4 tiles (one of them the threshold case), and
    /// a dry sentinel. Codec 2 is deliberately out of scope — the app decodes DEFLATE and never
    /// produces it, so no test writer should either.
    @Test
    func theWriterReproducesCommittedVectorsByteForByte() throws {
        for name in [
            "grid-raw-tile.obcg", "grid-rle-tile.obcg", "grid-rle-wins.obcg",
            "grid-minimal-dry.obcg",
        ] {
            let vector = try WeatherFixtures.vector(name)
            let header = try OBCGridCodec.validate(vector)

            // Read the object back out through the shipping decoder, tile by tile.
            let edge = Int(header.tileEdge)
            var cells = [UInt8](
                repeating: OBCPrecipitationTileCodec.noData,
                count: Int(header.width) * Int(header.height))
            for index in 0..<header.tileCount {
                let page = header.pageOfEntry(index)
                let pageOffset = try #require(header.pageOffset(page))
                let pageBytes = try #require(
                    vector.readBytes(at: pageOffset, count: header.pageBytes))
                let entry = try OBCGridCodec.decodeEntry(
                    page: pageBytes, indexInPage: index - page * Int(header.entriesPerPage))
                var payload = Data()
                if !entry.isDry {
                    let range = try OBCGridCodec.payloadRange(header: header, entry: entry)
                    payload = try #require(
                        vector.readBytes(at: range.lowerBound, count: range.count))
                }
                let tile = try OBCGridCodec.decodeTileCells(
                    header: header, entry: entry, payload: payload)
                for localRow in 0..<edge {
                    let row = (index / header.tileColumns) * edge + localRow
                    guard row < Int(header.height) else { continue }
                    for localColumn in 0..<edge {
                        let column = (index % header.tileColumns) * edge + localColumn
                        guard column < Int(header.width) else { continue }
                        cells[row * Int(header.width) + column] =
                            tile[localRow * edge + localColumn]
                    }
                }
            }

            let rewritten = OBCGridWriter.encode(OBCGridWriter.Spec(
                southMicrodegrees: header.southLatitudeMicrodegrees,
                westMicrodegrees: header.westLongitudeMicrodegrees,
                cellMicrodegrees: header.cellLatitudeStrideMicrodegrees,
                cellLongitudeMicrodegrees: header.cellLongitudeStrideMicrodegrees,
                width: header.width, height: header.height, tileEdge: header.tileEdge,
                entriesPerPage: header.entriesPerPage, cellSizeMetres: header.cellSizeMetres,
                productID: header.productID, tier: header.tier,
                validAt: Date(timeIntervalSince1970: TimeInterval(header.validAtUnixSeconds)),
                referenceTime: Date(
                    timeIntervalSince1970: TimeInterval(header.referenceTimeUnixSeconds)),
                observed: header.flags & OBCGridCodec.flagObserved != 0, cells: cells))
            #expect(rewritten == vector,
                    "\(name): the test writer and the Rust encoder must agree byte for byte")
        }
    }

    /// **The production tile geometry, with the multi-page directory arithmetic it implies.**
    ///
    /// Everything else in this suite runs at `tile_edge: 16 / entries_per_page: 8`, where a shard is
    /// one or two pages of small tiles; the shipping lattice is `256 / 128`, where a page boundary
    /// falls after 128 tiles of 65,536 cells each. A square lattice with 129 tiles would be 8.5
    /// million cells to synthesise, so this one is a single cell row 129 tiles wide — the cheapest
    /// shape that has two directory pages at the real numbers — and the corridor is placed across
    /// the boundary so both pages are read.
    @Test
    func aCorridorSpansTwoDirectoryPagesAtTheProductionTileGeometry() async throws {
        var builder = ManifestV2Builder()
        builder.tileEdge = 256
        builder.entriesPerPage = 128
        // 129 tile columns. The cell pitch is shrunk so 33,024 cells still land inside ±180°;
        // `cell_size_m` is nominal metadata and nothing here reads it as a ground truth.
        builder.cellMicrodegrees = 10
        builder.width = 129 * 256
        builder.height = 1
        builder.shardWidth = builder.width
        builder.shardHeight = 1
        builder.coveredRows = 0..<1
        builder.frames = [builder.frames[0]]

        let (service, http) = try Self.client(builder)
        // Cells 32,760...32,775 — the last of tile 127 and the first of tile 128, so the corridor
        // needs an entry from each directory page.
        let west = Int64(builder.latticeWestMicrodegrees) + 32_760 * 10
        let corridor = WeatherCorridor(bounds: WeatherBoundingBox(
            southMicrodegrees: Int64(builder.latticeSouthMicrodegrees),
            westMicrodegrees: west,
            northMicrodegrees: Int64(builder.latticeSouthMicrodegrees) + 10,
            eastMicrodegrees: west + 16 * 10))
        let selection = try #require(
            (try await service.precipitation(for: corridor, now: Self.now)).selection)
        let crop = try #require(selection.crops.first)
        #expect(crop.height == 1)
        for column in 0..<crop.width {
            #expect(crop.cells[column] == builder.cellValue(0, 32_760 + column, 0),
                    "cell \(column) across the page boundary")
        }

        // Two pages, and the byte ledger says so: at 128 entries per page the boundary is a
        // 1,540-byte step, and a client that guessed one page would decode the wrong entry.
        let key = builder.key(offsetMinutes: 0, shard: WeatherShardID(column: 0, row: 0))
        let header = try OBCGridCodec.decodeHeader(try #require(builder.objects()[key]))
        #expect(header.pageCount == 2)
        #expect(header.pageOfEntry(127) == 0 && header.pageOfEntry(128) == 1)
        let fetched = CorridorExtraction.coalesce(
            http.requests(forPathSuffix: "s0-0.obcg").compactMap(\.byteRange))
        let pages = [try #require(header.pageOffset(0)), try #require(header.pageOffset(1))]
        for page in pages {
            #expect(fetched.contains { $0.lowerBound <= page && $0.upperBound >= page + header.pageBytes },
                    "directory page at \(page) was never read")
        }
    }

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
