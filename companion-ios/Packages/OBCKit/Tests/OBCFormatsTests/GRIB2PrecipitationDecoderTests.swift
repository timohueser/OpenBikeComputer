import Foundation
import Testing
@testable import OBCFormats

@Suite("NOAA GFS GRIB2 precipitation subset")
struct GRIB2PrecipitationDecoderTests {
    @Test("decodes and de-duplicates the captured Manila APCP crop")
    func decodesCapturedSimplePackingField() throws {
        let data = try fixtureData()
        let grids = try GRIB2PrecipitationDecoder().decode(data)
        let grid = try #require(grids.first)

        #expect(grids.count == 1, "NOMADS returned two byte-identical cumulative fields")
        #expect(grid.referenceTime == GRIB2Timestamp(
            year: 2026, month: 8, day: 9, hour: 0, minute: 0, second: 0
        ))
        #expect(grid.startForecastHour == 0)
        #expect(grid.endForecastHour == 6)
        #expect(grid.width == 3)
        #expect(grid.height == 4)
        #expect(grid.latitudeOfFirstPointDegrees == 14.25)
        #expect(grid.longitudeOfFirstPointDegrees == 120.75)
        #expect(grid.latitudeOfLastPointDegrees == 15.0)
        #expect(grid.longitudeOfLastPointDegrees == 121.25)
        #expect(grid.longitudeIncrementDegrees == 0.25)
        #expect(grid.latitudeIncrementDegrees == 0.25)
        #expect(grid.scanningMode == 64)
        #expect(grid.valuesMM == [
            11.4375, 5.4375, 5.5,
            8.9375, 7.75, 8.8125,
            5.8125, 8.1875, 9.25,
            3.375, 5.8125, 6.5,
        ].map(Optional.some))
    }

    @Test("fails closed on a truncated message")
    func rejectsTruncation() throws {
        let complete = try fixtureData()
        let truncated = Data(complete.dropLast())

        do {
            _ = try GRIB2PrecipitationDecoder().decode(truncated)
            Issue.record("expected the truncated fixture to be rejected")
        } catch let error as GRIB2PrecipitationDecoderError {
            guard case .malformed = error else {
                Issue.record("expected malformed, got \(error)")
                return
            }
        }
    }

    @Test("rejects a contradictory PDT end timestamp")
    func rejectsMutatedEndTimestamp() throws {
        let data = try mutateFirstMessage(section: 4, offset: 34, bytes: be16(2_099))
        expectDecoderRejection(data)
    }

    @Test("rejects a scan mode that contradicts increasing latitude")
    func rejectsMutatedScanningMode() throws {
        let data = try mutateFirstMessage(section: 3, offset: 71, bytes: [0])
        expectDecoderRejection(data)
    }

    @Test("rejects a last longitude that contradicts the grid spacing")
    func rejectsMutatedLastLongitude() throws {
        let data = try mutateFirstMessage(
            section: 3,
            offset: 59,
            bytes: be32(130_000_000)
        )
        expectDecoderRejection(data)
    }

    @Test("rejects positive decimal-scale underflow instead of returning false dry")
    func rejectsPositiveDecimalScaleUnderflow() throws {
        let original = try firstMessageData()
        let rainy = try #require(GRIB2PrecipitationDecoder().decode(original).first)
        #expect(rainy.valuesMM.allSatisfy { ($0 ?? 0) > 0 })

        let mutated = try mutate(
            original,
            section: 5,
            offset: 17,
            bytes: signedMagnitude16(32_767)
        )
        #expect(mutated.count == original.count, "the reproducer changes only the exponent")
        expectDecoderRejection(mutated)
    }

    @Test("rejects both signs of extreme binary and decimal scales")
    func rejectsExtremePackingScales() throws {
        for (offset, exponent) in [
            (15, -32_767),
            (15, 32_767),
            (17, -32_767),
            (17, 32_767),
        ] {
            let data = try mutateFirstMessage(
                section: 5,
                offset: offset,
                bytes: signedMagnitude16(exponent)
            )
            expectDecoderRejection(data)
        }
    }

    @Test("accepts audited scale boundaries without producing zero or non-finite rain")
    func acceptsAuditedScaleBoundaries() throws {
        for (offset, exponent) in [
            (15, GRIB2PrecipitationDecoder.supportedBinaryScaleExponents.lowerBound),
            (15, GRIB2PrecipitationDecoder.supportedBinaryScaleExponents.upperBound),
            (17, GRIB2PrecipitationDecoder.supportedDecimalScaleExponents.lowerBound),
            (17, GRIB2PrecipitationDecoder.supportedDecimalScaleExponents.upperBound),
        ] {
            let data = try mutateFirstMessage(
                section: 5,
                offset: offset,
                bytes: signedMagnitude16(exponent)
            )
            let grid = try #require(GRIB2PrecipitationDecoder().decode(data).first)
            #expect(grid.valuesMM.allSatisfy { value in
                guard let value else { return false }
                return value.isFinite && value > 0
            })
        }
    }

    @Test("rejects each scale immediately outside the audited exponent window")
    func rejectsNeighboringScaleExponents() throws {
        for (offset, exponent) in [
            (15, GRIB2PrecipitationDecoder.supportedBinaryScaleExponents.lowerBound - 1),
            (15, GRIB2PrecipitationDecoder.supportedBinaryScaleExponents.upperBound + 1),
            (17, GRIB2PrecipitationDecoder.supportedDecimalScaleExponents.lowerBound - 1),
            (17, GRIB2PrecipitationDecoder.supportedDecimalScaleExponents.upperBound + 1),
        ] {
            let data = try mutateFirstMessage(
                section: 5,
                offset: offset,
                bytes: signedMagnitude16(exponent)
            )
            expectDecoderRejection(data)
        }
    }

    @Test("rejects a forecast range that contradicts its end timestamp")
    func rejectsMutatedForecastRange() throws {
        let data = try mutateFirstMessage(section: 4, offset: 49, bytes: be32(5))
        expectDecoderRejection(data)
    }

    @Test("rejects unsupported surface semantics")
    func rejectsMutatedSecondSurface() throws {
        let data = try mutateFirstMessage(section: 4, offset: 28, bytes: [1])
        expectDecoderRejection(data)
    }

    @Test("rejects no-bitmap count mismatches")
    func rejectsNoBitmapCountMismatch() throws {
        let data = try mutateFirstMessage(section: 5, offset: 5, bytes: be32(11))
        expectDecoderRejection(data)
    }

    @Test("decodes an exact inline bitmap and preserves missing cells")
    func decodesInlineBitmap() throws {
        let grids = try GRIB2PrecipitationDecoder().decode(inlineBitmapFixture())
        let grid = try #require(grids.first)

        #expect(grid.valuesMM.count == 12)
        #expect(grid.valuesMM[9] == 3.375)
        #expect(grid.valuesMM[10] == nil)
        #expect(grid.valuesMM[11] == nil)
    }

    @Test("rejects bitmap population/count mismatches")
    func rejectsBitmapPopulationMismatch() throws {
        let data = try mutate(
            inlineBitmapFixture(),
            section: 5,
            offset: 5,
            bytes: be32(9)
        )
        expectDecoderRejection(data)
    }

    @Test("rejects non-zero bitmap padding")
    func rejectsBitmapPadding() throws {
        let data = try mutate(
            inlineBitmapFixture(),
            section: 6,
            offset: 7,
            bytes: [0xc1]
        )
        expectDecoderRejection(data)
    }

    @Test("rejects payload bytes beyond the declared packed values")
    func rejectsPackedPayloadMismatch() throws {
        var bytes = [UInt8](try inlineBitmapFixture())
        let section7 = try sectionOffset(bytes, number: 7)
        bytes.insert(0, at: section7 + 15)
        write32(&bytes, at: section7, value: 16)
        write64(&bytes, at: 8, value: UInt64(bytes.count))
        expectDecoderRejection(Data(bytes))
    }

    @Test("rejects grids beyond the audited bbox allocation limit")
    func rejectsOversizedGridBeforeAllocation() throws {
        var bytes = [UInt8](try firstMessageData())
        let section3 = try sectionOffset(bytes, number: 3)
        write32(&bytes, at: section3 + 6, value: 2_052)
        write32(&bytes, at: section3 + 30, value: 513)
        write32(&bytes, at: section3 + 34, value: 4)
        expectDecoderRejection(Data(bytes))
    }

    @Test("rejects oversized inputs and excessive message counts")
    func rejectsContainerLimits() throws {
        expectDecoderRejection(Data(
            repeating: 0,
            count: GRIB2PrecipitationDecoder.maximumInputBytes + 1
        ))

        let message = try firstMessageData()
        var repeated = Data()
        for _ in 0 ... GRIB2PrecipitationDecoder.maximumMessageCount {
            repeated.append(message)
        }
        expectDecoderRejection(repeated)
    }

    @Test("optionally decodes every live reproduction crop with production code")
    func decodesOptInLiveCapture() throws {
        guard let directory = ProcessInfo.processInfo.environment[
            "OBC_WEATHER_LIVE_GFS_DIRECTORY"
        ] else { return }

        let root = URL(fileURLWithPath: directory, isDirectory: true)
        let cycle = try String(
            contentsOf: root.appendingPathComponent("selected-cycle.txt"),
            encoding: .utf8
        ).split(whereSeparator: \.isWhitespace)
        let date = try #require(cycle.first)
        let hourText = try #require(cycle.dropFirst().first)
        #expect(date.count == 8)
        let expectedReference = GRIB2Timestamp(
            year: try #require(Int(date.prefix(4))),
            month: try #require(Int(date.dropFirst(4).prefix(2))),
            day: try #require(Int(date.suffix(2))),
            hour: try #require(Int(hourText)),
            minute: 0,
            second: 0
        )

        for forecastHour in 1 ... 24 {
            let name = String(format: "f%03d.grib2", forecastHour)
            let data = try Data(contentsOf: root.appendingPathComponent(name))
            let grids = try GRIB2PrecipitationDecoder().decode(data)
            let grid = try #require(grids.first)
            #expect(grids.count == 1)
            #expect(grid.referenceTime == expectedReference)
            #expect(grid.startForecastHour == 0)
            #expect(grid.endForecastHour == forecastHour)
            #expect(grid.width == 3)
            #expect(grid.height == 4)
            #expect(grid.valuesMM.count == 12)
        }
    }

    private func fixtureData() throws -> Data {
        let url = try #require(Bundle.module.url(
            forResource: "gfs-manila-apcp-f006.grib2",
            withExtension: "b64",
            subdirectory: "Fixtures"
        ))
        let encoded = try String(contentsOf: url, encoding: .utf8)
        return try #require(Data(
            base64Encoded: encoded,
            options: .ignoreUnknownCharacters
        ))
    }

    private func firstMessageData() throws -> Data {
        let complete = try fixtureData()
        let bytes = [UInt8](complete)
        let length = try #require(Int(exactly: read64(bytes, at: 8)))
        return Data(bytes.prefix(length))
    }

    private func inlineBitmapFixture() throws -> Data {
        var bytes = [UInt8](try firstMessageData())
        let section5 = try sectionOffset(bytes, number: 5)
        let section6 = try sectionOffset(bytes, number: 6)
        let originalSection7 = try sectionOffset(bytes, number: 7)

        write32(&bytes, at: section5 + 5, value: 10)
        write32(&bytes, at: section6, value: 8)
        bytes[section6 + 5] = 0
        bytes.insert(contentsOf: [0xff, 0xc0], at: section6 + 6)
        let section7 = originalSection7 + 2
        write32(&bytes, at: section7, value: 15)
        bytes.removeSubrange(section7 + 15 ..< section7 + 17)
        write64(&bytes, at: 8, value: UInt64(bytes.count))
        return Data(bytes)
    }

    private func mutateFirstMessage(
        section: UInt8,
        offset: Int,
        bytes replacement: [UInt8]
    ) throws -> Data {
        try mutate(firstMessageData(), section: section, offset: offset, bytes: replacement)
    }

    private func mutate(
        _ data: Data,
        section: UInt8,
        offset: Int,
        bytes replacement: [UInt8]
    ) throws -> Data {
        var bytes = [UInt8](data)
        let base = try sectionOffset(bytes, number: section)
        bytes.replaceSubrange(base + offset ..< base + offset + replacement.count, with: replacement)
        return Data(bytes)
    }

    private func sectionOffset(_ bytes: [UInt8], number: UInt8) throws -> Int {
        let messageLength = try #require(Int(exactly: read64(bytes, at: 8)))
        var offset = 16
        while offset < messageLength - 4 {
            let length = Int(read32(bytes, at: offset))
            if bytes[offset + 4] == number { return offset }
            offset += length
        }
        Issue.record("missing section \(number)")
        return 0
    }

    private func expectDecoderRejection(_ data: Data) {
        do {
            _ = try GRIB2PrecipitationDecoder().decode(data)
            Issue.record("expected decoder rejection")
        } catch is GRIB2PrecipitationDecoderError {
            // Expected fail-closed behavior.
        } catch {
            Issue.record("unexpected error: \(error)")
        }
    }

    private func be16(_ value: UInt16) -> [UInt8] {
        [UInt8(value >> 8), UInt8(value & 0xff)]
    }

    private func signedMagnitude16(_ value: Int) -> [UInt8] {
        let magnitude = UInt16(abs(value))
        return be16(value < 0 ? magnitude | 0x8000 : magnitude)
    }

    private func be32(_ value: UInt32) -> [UInt8] {
        [
            UInt8(value >> 24),
            UInt8((value >> 16) & 0xff),
            UInt8((value >> 8) & 0xff),
            UInt8(value & 0xff),
        ]
    }

    private func read32(_ bytes: [UInt8], at offset: Int) -> UInt32 {
        UInt32(bytes[offset]) << 24
            | UInt32(bytes[offset + 1]) << 16
            | UInt32(bytes[offset + 2]) << 8
            | UInt32(bytes[offset + 3])
    }

    private func read64(_ bytes: [UInt8], at offset: Int) -> UInt64 {
        bytes[offset ..< offset + 8].reduce(0) { $0 << 8 | UInt64($1) }
    }

    private func write32(_ bytes: inout [UInt8], at offset: Int, value: UInt32) {
        bytes.replaceSubrange(offset ..< offset + 4, with: be32(value))
    }

    private func write64(_ bytes: inout [UInt8], at offset: Int, value: UInt64) {
        let encoded = (0 ..< 8).map { index in
            UInt8((value >> UInt64((7 - index) * 8)) & 0xff)
        }
        bytes.replaceSubrange(offset ..< offset + 8, with: encoded)
    }
}
