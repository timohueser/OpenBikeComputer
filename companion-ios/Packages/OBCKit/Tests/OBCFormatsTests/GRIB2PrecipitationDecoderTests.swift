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
}
