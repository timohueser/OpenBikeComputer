import Foundation
import OBCDomain
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// The OBCG → OBCW re-encode, and the hourly section's unit contract.
struct WeatherBundleBuilderTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    static let corridor = WeatherServiceClientTests.corridor

    static func request() -> WeatherRequest {
        WeatherRequest(
            requestID: 4_242,
            position: Coordinate(latitude: 47.27, longitude: 7.42),
            fixTime: now, bearingDegrees: 45, speedMetresPerSecond: 5.5)
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
        latitudeStride: UInt32 = 9_000, longitudeStride: UInt32 = 14_000,
        width: Int = 20, height: Int = 20, cellSizeMetres: UInt16 = 1_000,
        quality: PrecipitationQuality = .observed, seed: UInt8 = 0
    ) -> PrecipitationCrop {
        let cells = (0..<(width * height)).map { index in
            UInt8((index + Int(seed)) % 13)
        }
        return PrecipitationCrop(
            validAt: validAt, southMicrodegrees: south, westMicrodegrees: west,
            latitudeStrideMicrodegrees: latitudeStride, longitudeStrideMicrodegrees: longitudeStride,
            width: width, height: height, cellSizeMetres: cellSizeMetres, quality: quality,
            cells: cells)
    }

    static func selection(_ crops: [PrecipitationCrop]) -> PrecipitationSelection {
        PrecipitationSelection(
            productID: "dwd-rv", tier: .radar, nominalCellMetres: 1_000,
            attribution: WeatherAttribution(
                text: "Source: Deutscher Wetterdienst (DWD)",
                url: "https://creativecommons.org/licenses/by/4.0/"),
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(900), crops: crops)
    }

    private func build(
        precipitation: PrecipitationSelection?, hourly: HourlyForecast = hourly(),
        reason: NoRainMapReason? = nil, generation: UInt32 = 9
    ) throws -> BuiltWeatherBundle {
        try WeatherBundleBuilder().build(
            request: Self.request(), corridor: Self.corridor, hourly: hourly,
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

    @Test
    func theRainSectionIsACellForCellCopyOfTheCrop() throws {
        let crop = Self.crop()
        let built = try build(precipitation: Self.selection([crop]))
        let frame = try #require(built.bundle.rainFrames.first)
        #expect(frame.width == 20 && frame.height == 20)
        #expect(frame.cellSizeMetres == 1_000)
        #expect(frame.validAtUnixSeconds == Int64(crop.validAt.timeIntervalSince1970))
        #expect(built.bundle.bounds.southLatitudeMicrodegrees == 47_180_000)
        #expect(built.bundle.bounds.northLatitudeMicrodegrees == 47_180_000 + 20 * 9_000)
        #expect(built.bundle.bounds.eastLongitudeMicrodegrees == 7_280_000 + 20 * 14_000)
        #expect(built.bundle.bounds.gridOriginLatitudeMicrodegrees
            == built.bundle.bounds.southLatitudeMicrodegrees)

        // Rebuild the grid out of the 16 x 16 tiles and compare it to the crop, cell for cell.
        let edge = OBCPrecipitationTileCodec.tileEdge
        let tileColumns = (Int(frame.width) + edge - 1) / edge
        for row in 0..<Int(frame.height) {
            for column in 0..<Int(frame.width) {
                let tile = (row / edge) * tileColumns + (column / edge)
                let value = frame.tiles[tile][(row % edge) * edge + (column % edge)]
                #expect(value == crop.cells[row * crop.width + column])
            }
        }
        // Padding outside the declared grid is the no-data intensity, never dry (OBCW §5).
        let lastTile = try #require(frame.tiles.last)
        #expect(lastTile[edge * edge - 1] == OBCPrecipitationTileCodec.noData)
        #expect(frame.quality.contains(.observed))
        // Tile padding lies *outside* the declared grid, so it is not "partial coverage" — that
        // flag means in-bounds cells are unavailable, and claiming it here would make every frame
        // whose width is not a multiple of 16 look degraded.
        #expect(!frame.quality.contains(.partialCoverage))
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

    /// A frame whose lattice cannot tile the common window is **dropped and counted**, never
    /// resampled onto it. A sub-cell shift is exactly the fabricated precision the epic forbids.
    @Test
    func anIncompatibleLatticeIsDroppedRatherThanResampled() throws {
        let fine = Self.crop(validAt: Self.now)
        let coarse = Self.crop(
            validAt: Self.now.addingTimeInterval(3_600), south: 47_000_000, west: 7_000_000,
            latitudeStride: 62_500, longitudeStride: 62_500, width: 16, height: 16,
            cellSizeMetres: 6_500, quality: .forecast, seed: 2)
        let built = try build(precipitation: Self.selection([fine, coarse]))
        #expect(built.bundle.rainFrames.count == 1)
        #expect(built.bundle.rainFrames[0].cellSizeMetres == 6_500, "the coarse window survives")
        #expect(built.state.diagnostics.droppedIncompatibleFrames == 1)
    }

    /// The finer frames of a product whose lattices *do* nest keep all their detail.
    @Test
    func aNestedFinerLatticeIsKeptAtFullResolution() throws {
        let coarse = Self.crop(
            validAt: Self.now, south: 47_180_000, west: 7_280_000,
            latitudeStride: 27_000, longitudeStride: 42_000, width: 6, height: 6,
            cellSizeMetres: 3_000, quality: .forecast)
        let fine = Self.crop(
            validAt: Self.now.addingTimeInterval(900), south: 47_180_000, west: 7_280_000,
            latitudeStride: 9_000, longitudeStride: 14_000, width: 18, height: 18, seed: 4)
        let built = try build(precipitation: Self.selection([coarse, fine]))
        #expect(built.bundle.rainFrames.count == 2)
        #expect(built.bundle.rainFrames[0].width == 6)
        #expect(built.bundle.rainFrames[1].width == 18, "three fine cells per coarse cell")
        #expect(built.state.diagnostics.droppedIncompatibleFrames == 0)
    }

    @Test
    func anOversizeCorridorShrinksTheWindowBeforeDroppingFrames() throws {
        // Nine 15-minute frames over a 400 x 400-cell corridor is far past the 64 KiB policy.
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                width: 400, height: 400, seed: UInt8(index))
        }
        let built = try build(precipitation: Self.selection(crops))
        #expect(built.bytes.count <= OBCWeatherCodec.producerPolicyMaximumLength)
        #expect(built.bundle.rainFrames.count == 9, "every timestamp survives; the window shrinks")
        #expect(built.state.diagnostics.droppedOversizeFrames == 0)
        let frame = try #require(built.bundle.rainFrames.first)
        #expect(frame.width < 400)
        // The rider's own cell stays inside the shrunken window.
        let bounds = built.bundle.bounds
        #expect(bounds.southLatitudeMicrodegrees <= 47_270_000)
        #expect(bounds.northLatitudeMicrodegrees >= 47_270_000)
        #expect(bounds.westLongitudeMicrodegrees <= 7_420_000)
        #expect(bounds.eastLongitudeMicrodegrees >= 7_420_000)
    }

    /// Regression, adversarial review finding 1: the shrink must keep the **rider** inside, not the
    /// corridor's midpoint.
    ///
    /// A corridor is projected *ahead* of the rider, so for anyone moving quickly its midpoint sits
    /// tens of kilometres up the road. Anchoring the shrink there walked the window off the back of
    /// the rider entirely — the bundle carried rain for where they were going and none at all for
    /// where they were.
    @Test
    func theShrinkKeepsTheRiderInsideNotTheCorridorMidpoint() throws {
        let rider = Coordinate(latitude: 47.19, longitude: 7.29)
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                width: 400, height: 400, seed: UInt8(index))
        }
        let bounds = crops[0].bounds
        let request = WeatherRequest(
            requestID: 1, position: rider, fixTime: Self.now, bearingDegrees: 0,
            speedMetresPerSecond: 15)
        let built = try WeatherBundleBuilder().build(
            request: request, corridor: WeatherCorridor(bounds: bounds, isUndirected: false),
            hourly: Self.hourly(), precipitation: Self.selection(crops), noRainMapReason: nil,
            generation: 1, now: Self.now)

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

    /// With no fix there is no rider cell, so the corridor's midpoint is the honest fallback.
    @Test
    func withoutAFixTheShrinkFallsBackToTheCorridorMidpoint() throws {
        let crops = (0..<9).map { index in
            Self.crop(
                validAt: Self.now.addingTimeInterval(TimeInterval(index) * 900),
                width: 400, height: 400, seed: UInt8(index))
        }
        let bounds = crops[0].bounds
        let built = try WeatherBundleBuilder().build(
            request: WeatherRequest(requestID: 1),
            corridor: WeatherCorridor(bounds: bounds, isUndirected: true),
            hourly: Self.hourly(), precipitation: Self.selection(crops), noRainMapReason: nil,
            generation: 1, now: Self.now)
        let window = built.bundle.bounds
        let midpoint = (bounds.southMicrodegrees + bounds.northMicrodegrees) / 2
        #expect(Int64(window.southLatitudeMicrodegrees) <= midpoint)
        #expect(Int64(window.northLatitudeMicrodegrees) > midpoint)
    }

    // MARK: - The two halves are independent

    @Test
    func anAbsentRainProductNeverDiscardsTheHourlyForecast() throws {
        let built = try build(precipitation: nil, reason: .corridorNotCovered)
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .corridorNotCovered)
        #expect(built.state.precipitation == nil)
        #expect(built.state.attributions == [.met])
        // The bundle still states the region it describes.
        #expect(built.bundle.bounds.southLatitudeMicrodegrees
            == Int32(Self.corridor.bounds.southMicrodegrees))
    }

    @Test
    func bothAttributionsSurviveWhenBothHalvesAnswered() throws {
        let built = try build(precipitation: Self.selection([Self.crop()]))
        #expect(built.state.attributions.count == 2)
        #expect(built.state.attributions.first == .met)
        #expect(built.state.attributions.last?.text == "Source: Deutscher Wetterdienst (DWD)")
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
