import Foundation
import OBCDomain
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// The OBCG → OBCW re-encode, the uniform east–west resample, and the hourly section's unit contract.
struct WeatherBundleBuilderTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    /// The canonical lattice's cell: 0.01° on both axes, ~1,113 m north–south at every latitude.
    static let cell: Int64 = 10_000
    static let cellSizeMetres: UInt16 = 1_113

    static let rider = Coordinate(latitude: 47.27, longitude: 7.42)
    static let corridor = WeatherCorridor(bounds: WeatherBoundingBox(
        southMicrodegrees: 47_180_000, westMicrodegrees: 7_280_000,
        northMicrodegrees: 47_380_000, eastMicrodegrees: 7_480_000))

    static func request() -> WeatherRequest {
        WeatherRequest(requestID: 4_242, position: rider, fixTime: now)
    }

    /// 24 hours with a deliberate mix of present and absent optionals.
    static func hourly(from base: Date = now) -> HourlyForecast {
        let hours: [HourlyCondition] = (0..<24).map { index in
            let validAt = base.addingTimeInterval(TimeInterval(index) * 3_600)
            let temperature: Double = 17.4 + Double(index) * 0.1
            let amount: Double? = index == 3 ? nil : Double(index) * 0.25
            let probability: Double? = index == 4 ? nil : 1.4 + Double(index)
            let condition: OBCWeatherCondition = index == 5 ? .thunderstorm : .overcast
            let windFrom: Double? = index == 6 ? nil : 189
            let gust: Double? = index == 7 ? nil : 9.8
            return HourlyCondition(
                validAt: validAt, temperatureCelsius: temperature,
                precipitationMillimetres: amount, precipitationProbabilityPercent: probability,
                condition: condition, windFromDegrees: windFrom,
                windSpeedMetresPerSecond: 5.5, windGustMetresPerSecond: gust)
        }
        return HourlyForecast(hours: hours, attribution: .met, retrievedAt: base)
    }

    static func crop(
        validAt: Date = now, south: Int64 = 47_180_000, west: Int64 = 7_280_000,
        width: Int = 20, height: Int = 20,
        quality: PrecipitationQuality = .observed, seed: UInt8 = 0
    ) -> PrecipitationCrop {
        let cells = (0..<(width * height)).map { index in UInt8((index + Int(seed)) % 13) }
        return PrecipitationCrop(
            validAt: validAt, southMicrodegrees: south, westMicrodegrees: west,
            latitudeStrideMicrodegrees: UInt32(cell), longitudeStrideMicrodegrees: UInt32(cell),
            width: width, height: height, cellSizeMetres: cellSizeMetres, quality: quality,
            cells: cells)
    }

    static func selection(_ crops: [PrecipitationCrop]) -> PrecipitationSelection {
        PrecipitationSelection(
            generation: "20260810T1430Z", nominalCellMetres: cellSizeMetres,
            attributions: [WeatherAttribution(
                text: "Source: Deutscher Wetterdienst (DWD)",
                url: "https://creativecommons.org/licenses/by/4.0/", sourceID: "dwd-rv")],
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(900), crops: crops)
    }

    private func build(
        precipitation: PrecipitationSelection?, hourly: HourlyForecast = hourly(),
        reason: NoRainMapReason? = nil, generation: UInt32 = 9,
        request: WeatherRequest = request(), corridor: WeatherCorridor = corridor
    ) throws -> BuiltWeatherBundle {
        try WeatherBundleBuilder().build(
            request: request, corridor: corridor, hourly: hourly,
            precipitation: precipitation, noRainMapReason: reason, generation: generation,
            now: Self.now)
    }

    // MARK: - Determinism

    @Test
    func theSameInputsProduceByteIdenticalBundles() throws {
        let selection = Self.selection([Self.crop(), Self.crop(
            validAt: Self.now.addingTimeInterval(900), seed: 3)])
        let first = try build(precipitation: selection)
        let second = try build(precipitation: selection)
        #expect(first.bytes == second.bytes)
        // And the bytes survive a decode/re-encode through the wire codec unchanged, which is what
        // makes the device's generation/CRC comparison meaningful.
        #expect(try OBCWeatherCodec.encode(OBCWeatherCodec.decode(first.bytes)) == first.bytes)
        #expect(first.bytes.count <= OBCWeatherCodec.producerPolicyMaximumLength)
    }

    // MARK: - Hourly section

    @Test
    func hourlyUnitsAndUnavailableSentinelsArePinned() throws {
        let built = try build(precipitation: nil)
        let records = built.bundle.hourly
        #expect(records.count == 24)
        #expect(built.bundle.validFromUnixSeconds == Int64(Self.now.timeIntervalSince1970))
        #expect(records[0].validTimeOffsetSeconds == 0)
        #expect(records[23].validTimeOffsetSeconds == 23 * 3_600)

        #expect(records[0].temperatureDeciCelsius == 174)
        #expect(records[0].windSpeedDeciMetresPerSecond == 55)
        #expect(records[0].windGustDeciMetresPerSecond == 98)
        #expect(records[0].windFromDegrees == 189)
        #expect(records[0].precipitationProbabilityPercent == 1, "1.4 % rounds to 1")
        #expect(records[0].precipitationTenthMillimetres == 0)
        #expect(records[2].precipitationTenthMillimetres == 5, "0.5 mm is five tenths")
        #expect(records[5].condition == .thunderstorm)

        // Unavailable stays unavailable. Zero would mean "no rain", "no wind", "no chance".
        #expect(records[3].precipitationTenthMillimetres == UInt16.max)
        #expect(records[4].precipitationProbabilityPercent == UInt8.max)
        #expect(records[6].windFromDegrees == UInt16.max)
        #expect(records[7].windGustDeciMetresPerSecond == UInt16.max)
    }

    @Test
    func outOfRangeAndWrappingValuesAreHandledWithoutInventingNumbers() throws {
        var forecast = Self.hourly()
        forecast.hours[0].temperatureCelsius = 900          // beyond the wire's range
        forecast.hours[1].windFromDegrees = 360             // a full turn is north
        forecast.hours[2].windFromDegrees = -45             // folds into the circle
        forecast.hours[3].windSpeedMetresPerSecond = 400    // implausible; not clamped to 200
        let built = try build(precipitation: nil, hourly: forecast)
        #expect(built.bundle.hourly[0].temperatureDeciCelsius == Int16.min)
        #expect(built.bundle.hourly[1].windFromDegrees == 0)
        #expect(built.bundle.hourly[2].windFromDegrees == 315)
        #expect(built.bundle.hourly[3].windSpeedDeciMetresPerSecond == UInt16.max)
    }

    @Test
    func anIncompleteHourlyForecastIsARefusal() throws {
        var forecast = Self.hourly()
        forecast.hours.removeLast()
        #expect(throws: WeatherBundleBuildError.hourlyUnusable) {
            try build(precipitation: nil, hourly: forecast)
        }
        var misaligned = Self.hourly()
        misaligned.hours[9].validAt = misaligned.hours[9].validAt.addingTimeInterval(60)
        #expect(throws: WeatherBundleBuildError.hourlyUnusable) {
            try build(precipitation: nil, hourly: misaligned)
        }
    }

    // MARK: - Rain section

    /// At the equator the resample is the identity, so this is the copy test the old cell-for-cell
    /// one was: rows 1:1, columns 1:1, every value the crop's own.
    @Test
    func atTheEquatorTheRainSectionIsACellForCellCopyOfTheCrop() throws {
        let crop = Self.crop(south: -100_000, west: 200_000)
        let corridor = WeatherCorridor(bounds: crop.bounds)
        let request = WeatherRequest(
            requestID: 1, position: Coordinate(latitude: 0.0, longitude: 0.3), fixTime: Self.now)
        let built = try build(
            precipitation: Self.selection([crop]), request: request, corridor: corridor)
        let frame = try #require(built.bundle.rainFrames.first)
        #expect(frame.width == 20 && frame.height == 20, "cos(0) = 1: no columns are dropped")
        #expect(frame.cellSizeMetres == Self.cellSizeMetres)
        #expect(frame.validAtUnixSeconds == Int64(crop.validAt.timeIntervalSince1970))
        #expect(built.bundle.bounds.southLatitudeMicrodegrees == -100_000)
        #expect(built.bundle.bounds.northLatitudeMicrodegrees == -100_000 + 20 * 10_000)
        #expect(built.bundle.bounds.eastLongitudeMicrodegrees == 200_000 + 20 * 10_000)
        #expect(built.bundle.bounds.gridOriginLatitudeMicrodegrees
            == built.bundle.bounds.southLatitudeMicrodegrees)

        let edge = OBCPrecipitationTileCodec.tileEdge
        let tileColumns = (Int(frame.width) + edge - 1) / edge
        for row in 0..<Int(frame.height) {
            for column in 0..<Int(frame.width) {
                let tile = (row / edge) * tileColumns + (column / edge)
                #expect(frame.tiles[tile][(row % edge) * edge + (column % edge)]
                    == crop.cells[row * crop.width + column])
            }
        }
        // Padding outside the declared grid is the no-data intensity, never dry (OBCW §5).
        #expect(try #require(frame.tiles.last)[edge * edge - 1]
            == OBCPrecipitationTileCodec.noData)
        #expect(frame.quality.contains(.observed))
        // Tile padding lies *outside* the declared grid, so it is not "partial coverage" — that flag
        // means in-bounds cells are unavailable, and claiming it here would make every frame whose
        // width is not a multiple of 16 look degraded.
        #expect(!frame.quality.contains(.partialCoverage))
    }

    /// **The resample is nearest neighbour on an integer map, and every output column is a real
    /// source column.** No averaging, no maximum-of-a-group, no interpolation: a value that reaches
    /// the device is a value a source measured.
    @Test
    func theResampleIsNearestNeighbourAndDropsColumnsRatherThanMergingThem() throws {
        let crop = Self.crop(width: 32, height: 8)
        let corridor = WeatherCorridor(bounds: crop.bounds)
        let built = try build(precipitation: Self.selection([crop]), corridor: corridor)
        let frame = try #require(built.bundle.rainFrames.first)
        let cosine = Foundation.cos(Self.rider.latitude * .pi / 180)
        #expect(frame.width == UInt16((32.0 * cosine).rounded()))
        #expect(frame.height == 8, "rows are untouched")

        let edge = OBCPrecipitationTileCodec.tileEdge
        let tileColumns = (Int(frame.width) + edge - 1) / edge
        for output in 0..<Int(frame.width) {
            let source = ((2 * output + 1) * 32) / (2 * Int(frame.width))
            for row in 0..<8 {
                let tile = (row / edge) * tileColumns + (output / edge)
                #expect(frame.tiles[tile][(row % edge) * edge + (output % edge)]
                    == crop.cells[row * 32 + source],
                    "output column \(output) must be source column \(source) verbatim")
            }
        }
    }

    /// **The normalisation guard (#1254 phase 4a-norm), two-sided and driven by the shared vector.**
    ///
    /// Every number asserted here comes out of `specs/vectors/manifest.json`'s
    /// `wx_manifest_v2.resample_equivalence`, which is the same file
    /// `host/obc-wx-client/tests/client.rs::the_resampled_corridor_matches_the_shared_vector_at_every_latitude`
    /// reads — so "the two clients agree to the byte" is a checked fact rather than two suites
    /// happening to print the same numbers. The block's `scenario` string is normative and this is
    /// its Swift implementation.
    ///
    /// Three things are pinned, because no two of them are enough:
    ///
    /// - **bytes**, against the row and against a 200 KiB budget — the mechanism was chosen for it;
    /// - **the pitch**, because the rejected `round(1/cos φ)` max-merge also fit a byte budget and
    ///   what killed it was 1,428 m cells at Frankfurt and a 2x step across 48.19 °N;
    /// - **the decoded cell image**, because at the raw4 worst case every tile is 128 bytes whatever
    ///   is in it, so a half-cell drift in the nearest-neighbour column map would move which cells a
    ///   rider sees while every length and every budget still passed.
    ///
    /// A mismatch here is a **cross-language divergence**, not a number to update: the fixture is
    /// Rust's measurement and the two readers are supposed to be mirrors.
    @Test
    func aNinetyKilometreDiscMatchesTheSharedResampleVectorAtEveryLatitude() throws {
        let northSouthPitch = 0.01 * 111_320.0  // 1,113.2 m, the lattice's own cell height
        let directory = try JSONSerialization.jsonObject(
            with: ManifestV2Tests.repositoryFile("specs/vectors/manifest.json")) as! [String: Any]
        let block = try #require(directory["wx_manifest_v2"] as? [String: Any])
        let vector = try #require(block["resample_equivalence"] as? [String: Any])
        let rows = try #require(vector["rows"] as? [[String: Any]])
        #expect(rows.count >= 9, "the sweep must keep its latitudes")

        for row in rows {
            let latitude = try #require(row["lat_deg"] as? Double)
            let rider = Coordinate(latitude: latitude, longitude: 7.9)
            let request = WeatherRequest(requestID: 1, position: rider, fixTime: Self.now)
            let corridor = try #require(WeatherCorridor.around(request))

            // One crop per frame covering the whole lattice-aligned corridor window, filled with
            // deliberately incompressible texture: a uniform field would RLE4 down to nothing and
            // the byte budget would pass without measuring anything. The window arithmetic is the
            // scenario's, cell for cell, and it is the Rust helper's.
            let bounds = corridor.bounds
            let south = floorDivide(bounds.southMicrodegrees, Self.cell) * Self.cell
            let west = floorDivide(bounds.westMicrodegrees, Self.cell) * Self.cell
            let height = Int(floorDivide(bounds.northMicrodegrees - south, Self.cell) + 1)
            let width = Int(floorDivide(bounds.eastMicrodegrees - west, Self.cell) + 1)
            var crops: [PrecipitationCrop] = []
            for index in 0..<9 {
                var cells = [UInt8](repeating: 0, count: width * height)
                for offset in 0..<cells.count {
                    cells[offset] = UInt8((offset &* 7 &+ index) % 13)
                }
                crops.append(PrecipitationCrop(
                    validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                    southMicrodegrees: south, westMicrodegrees: west,
                    latitudeStrideMicrodegrees: UInt32(Self.cell),
                    longitudeStrideMicrodegrees: UInt32(Self.cell),
                    width: width, height: height, cellSizeMetres: Self.cellSizeMetres,
                    quality: index == 0 ? .observed : .forecast, cells: cells))
            }
            let built = try build(
                precipitation: Self.selection(crops), request: request, corridor: corridor)
            let frame = try #require(built.bundle.rainFrames.first)
            let window = built.bundle.bounds
            let site = "\(latitude) N"

            // The source lattice columns the window spans — read back out of the *bundle*, since
            // the window's east edge is `west + sourceColumns * cell` by construction.
            let sourceColumns = Int(
                (Int64(window.eastLongitudeMicrodegrees)
                    - Int64(window.westLongitudeMicrodegrees)) / Self.cell)
            #expect(sourceColumns == (try #require(row["source_columns"] as? Int)),
                    "\(site): source columns")
            let pinnedWindow = try #require(row["window"] as? [Int])
            #expect(Int(frame.width) == pinnedWindow[0], "\(site): output width")
            #expect(Int(frame.height) == pinnedWindow[1], "\(site): output height")
            #expect(built.bytes.count == (try #require(row["bundle_bytes"] as? Int)),
                    "\(site): bundle length")
            #expect(Self.cellsHash(frame) == (try #require(row["frame0_cells_fnv1a64"] as? String)),
                    "\(site): the decoded cell image moved")

            // The guard the mechanism was chosen for, both sides of it. The pitch is read off the
            // bundle's own declared window rather than off the builder's intention.
            #expect(built.bytes.count <= 200 * 1_024, "\(site): byte budget")
            #expect(built.bundle.rainFrames.count == 9,
                    "\(site): every timestamp must survive without the shrink loop firing")
            let spanDegrees = Double(
                Int64(window.eastLongitudeMicrodegrees)
                    - Int64(window.westLongitudeMicrodegrees)) / 1_000_000
            let pitch = spanDegrees * 111_320 * Foundation.cos(latitude * .pi / 180)
                / Double(frame.width)
            #expect(abs(pitch - northSouthPitch) / northSouthPitch < 0.02,
                    "\(site): \(Int(pitch.rounded())) m east-west against \(northSouthPitch) m")
            #expect(frame.cellSizeMetres == Self.cellSizeMetres,
                    "\(site): the frame states the lattice's own cell size")
        }
    }

    /// FNV-1a 64 over one decoded frame's cells, row-major with row 0 south — the hash
    /// `resample_equivalence` pins, defined by its `cells_hash` note.
    ///
    /// Chosen because it is four lines in any language: the whole point of the row is that Swift
    /// computes the same number over the same cells as Rust does, and a hash needing a library would
    /// have been a hash one of the two suites quietly skipped. The cells come back out of the
    /// *encoded bundle* through the wire decoder, so this measures what a device would read.
    static func cellsHash(_ frame: OBCWeatherRainFrame) -> String {
        let edge = OBCPrecipitationTileCodec.tileEdge
        let width = Int(frame.width)
        let height = Int(frame.height)
        let tileColumns = (width + edge - 1) / edge
        var cells = [UInt8](repeating: OBCPrecipitationTileCodec.noData, count: width * height)
        for (index, tile) in frame.tiles.enumerated() {
            let tileColumn = index % tileColumns
            let tileRow = index / tileColumns
            for localRow in 0..<edge {
                let row = tileRow * edge + localRow
                guard row < height else { continue }
                for localColumn in 0..<edge {
                    let column = tileColumn * edge + localColumn
                    guard column < width else { continue }
                    cells[row * width + column] = tile[localRow * edge + localColumn]
                }
            }
        }
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for cell in cells {
            hash ^= UInt64(cell)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        return String(format: "%016llx", hash)
    }

    @Test
    func genuineTimestampsSurviveIncludingALatentObservation() throws {
        // An observation from four hours before the hourly base — OBCW §5 allows exactly this, and
        // re-stamping it to look current is forbidden.
        let latent = Self.crop(validAt: Self.now.addingTimeInterval(-4 * 3_600))
        let forward = Self.crop(validAt: Self.now.addingTimeInterval(3_600), seed: 5)
        let built = try build(precipitation: Self.selection([forward, latent]))
        #expect(built.bundle.rainFrames.map(\.validAtUnixSeconds)
            == [Int64(latent.validAt.timeIntervalSince1970),
                Int64(forward.validAt.timeIntervalSince1970)])
        #expect(built.bundle.rainFrames[0].validAtUnixSeconds < built.bundle.validFromUnixSeconds)
    }

    /// With one lattice **no frame can fail to tile the window**, so none is ever dropped. The two
    /// nesting tests this replaces asserted the opposite behaviour for a heterogeneous product set
    /// that no longer exists (#1244); asserting the *absence* of a drop is what keeps a resampling
    /// or dropping branch from creeping back in.
    @Test
    func everyFrameOfOneDatasetSurvivesTheWindow() throws {
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                seed: UInt8(index))
        }
        let built = try build(precipitation: Self.selection(crops))
        #expect(built.bundle.rainFrames.count == 9)
        #expect(built.state.diagnostics.droppedOversizeFrames == 0)
        let widths = Set(built.bundle.rainFrames.map(\.width))
        let heights = Set(built.bundle.rainFrames.map(\.height))
        #expect(widths.count == 1 && heights.count == 1,
                "one lattice, one window: every frame is the same shape")
    }

    @Test
    func anOversizeCorridorShrinksTheWindowBeforeDroppingFrames() throws {
        // Nine 15-minute frames over a 900 x 900-cell corridor is far past the producer policy even
        // after the resample.
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                width: 900, height: 900, seed: UInt8(index))
        }
        let corridor = WeatherCorridor(bounds: crops[0].bounds)
        let built = try build(precipitation: Self.selection(crops), corridor: corridor)
        #expect(built.bytes.count <= OBCWeatherCodec.producerPolicyMaximumLength)
        #expect(built.bundle.rainFrames.count == 9, "every timestamp survives; the window shrinks")
        #expect(built.state.diagnostics.droppedOversizeFrames == 0)
        #expect(try #require(built.bundle.rainFrames.first).width < 900)
        // The rider's own cell stays inside the shrunken window.
        let bounds = built.bundle.bounds
        #expect(Int64(bounds.southLatitudeMicrodegrees) <= Self.rider.latitudeMicrodegrees)
        #expect(Int64(bounds.northLatitudeMicrodegrees) > Self.rider.latitudeMicrodegrees)
        #expect(Int64(bounds.westLongitudeMicrodegrees) <= Self.rider.longitudeMicrodegrees)
        #expect(Int64(bounds.eastLongitudeMicrodegrees) > Self.rider.longitudeMicrodegrees)
        // Shrinking happens in **source** cells, so the window's corners stay lattice-aligned.
        #expect((Int64(bounds.southLatitudeMicrodegrees) - 47_180_000) % Self.cell == 0)
        #expect((Int64(bounds.westLongitudeMicrodegrees) - 7_280_000) % Self.cell == 0)
        #expect((Int64(bounds.northLatitudeMicrodegrees)
            - Int64(bounds.southLatitudeMicrodegrees)) % Self.cell == 0)
    }

    /// Regression, adversarial review finding 1: the shrink must keep the **rider** inside, not the
    /// corridor's midpoint.
    @Test
    func theShrinkKeepsTheRiderInsideNotTheCorridorMidpoint() throws {
        let rider = Coordinate(latitude: 47.19, longitude: 7.29)
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                width: 900, height: 900, seed: UInt8(index))
        }
        let bounds = crops[0].bounds
        let built = try build(
            precipitation: Self.selection(crops),
            request: WeatherRequest(requestID: 1, position: rider, fixTime: Self.now),
            corridor: WeatherCorridor(bounds: bounds))

        #expect(built.bytes.count <= OBCWeatherCodec.producerPolicyMaximumLength)
        #expect(built.bundle.rainFrames.count == 9, "every timestamp is still answered")
        let window = built.bundle.bounds
        #expect(Int64(window.southLatitudeMicrodegrees) <= rider.latitudeMicrodegrees)
        #expect(Int64(window.northLatitudeMicrodegrees) > rider.latitudeMicrodegrees)
        #expect(Int64(window.westLongitudeMicrodegrees) <= rider.longitudeMicrodegrees)
        #expect(Int64(window.eastLongitudeMicrodegrees) > rider.longitudeMicrodegrees)
        // And this really is the oversize path: the window did shrink.
        #expect(Int64(window.northLatitudeMicrodegrees) < bounds.northMicrodegrees)
    }

    /// With no fix there is no rider cell, so the corridor's midpoint is the honest fallback — for
    /// the shrink anchor *and* for the latitude the resample is computed at.
    @Test
    func withoutAFixTheWindowFallsBackToTheCorridorMidpoint() throws {
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                width: 900, height: 900, seed: UInt8(index))
        }
        let bounds = crops[0].bounds
        let built = try build(
            precipitation: Self.selection(crops), request: WeatherRequest(requestID: 1),
            corridor: WeatherCorridor(bounds: bounds))
        let window = built.bundle.bounds
        let midpoint = (bounds.southMicrodegrees + bounds.northMicrodegrees) / 2
        #expect(Int64(window.southLatitudeMicrodegrees) <= midpoint)
        #expect(Int64(window.northLatitudeMicrodegrees) > midpoint)
    }

    // MARK: - The two halves are independent

    @Test
    func anAbsentRainMapNeverDiscardsTheHourlyForecast() throws {
        let built = try build(precipitation: nil, reason: .outOfDomain)
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .outOfDomain)
        #expect(built.state.precipitation == nil)
        #expect(built.state.attributions == [.met])
        // The bundle still states the region it describes.
        #expect(built.bundle.bounds.southLatitudeMicrodegrees
            == Int32(Self.corridor.bounds.southMicrodegrees))
    }

    /// A **dry** map is not an absent one: nine all-zero frames still ship, still carry the dataset's
    /// credit, and carry no ``NoRainMapReason`` at all.
    @Test
    func aFullyDryTimelineIsNineRealFramesNotAnAbsentRainMap() throws {
        let crops = (0..<9).map { index in
            PrecipitationCrop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                southMicrodegrees: 47_180_000, westMicrodegrees: 7_280_000,
                latitudeStrideMicrodegrees: UInt32(Self.cell),
                longitudeStrideMicrodegrees: UInt32(Self.cell),
                width: 20, height: 20, cellSizeMetres: Self.cellSizeMetres, quality: .observed,
                cells: [UInt8](repeating: OBCPrecipitationTileCodec.dry, count: 400))
        }
        let built = try build(precipitation: Self.selection(crops))
        #expect(built.bundle.rainFrames.count == 9)
        #expect(built.state.noRainMapReason == nil)
        for frame in built.bundle.rainFrames {
            let declared = Int(frame.width) * Int(frame.height)
            let dry = frame.tiles.flatMap { $0 }
                .filter { $0 == OBCPrecipitationTileCodec.dry }.count
            #expect(dry == declared, "every declared cell is intensity 0")
            #expect(!frame.quality.contains(.partialCoverage))
        }
    }

    @Test
    func bothAttributionsSurviveWhenBothHalvesAnswered() throws {
        let built = try build(precipitation: Self.selection([Self.crop()]))
        #expect(built.state.attributions.count == 2)
        #expect(built.state.attributions.first == .met)
        #expect(built.state.attributions.last?.sourceID == "dwd-rv")
        #expect(built.state.noRainMapReason == nil)
    }

    @Test
    func theRequestIdAndGenerationRideTheHeader() throws {
        let built = try build(precipitation: nil, generation: 77)
        #expect(built.bundle.requestID == 4_242)
        #expect(built.bundle.generation == 77)
        #expect(built.bundle.generatedAtUnixSeconds == Int64(Self.now.timeIntervalSince1970))
        #expect(built.bundle.validUntilUnixSeconds
            >= built.bundle.validFromUnixSeconds + 24 * 3_600)
    }
}
