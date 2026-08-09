import Foundation
import Testing

@Suite("WX1 captured source contracts")
struct WeatherFixtureContractTests {
    @Test("MET fixture contains 24 complete, ordered hourly records")
    func metHourlyContract() throws {
        let root = try fixtureJSON("met-locationforecast-oslo-24h")
        let hours = try #require(root["hours"] as? [[String: Any]])
        #expect(hours.count == 24)

        let required: Set<String> = [
            "time", "air_temperature_c", "precipitation_amount_mm",
            "probability_of_precipitation_percent", "symbol_code",
            "wind_from_direction_degrees", "wind_speed_mps", "wind_gust_mps",
        ]
        #expect(hours.allSatisfy { required.isSubset(of: Set($0.keys)) })
        let timestamps = try hours.map { try #require($0["time"] as? String) }
        #expect(timestamps == timestamps.sorted())
        #expect(Set(hours.compactMap { $0["symbol_code"] as? String }).contains("heavyrain"))
    }

    @Test("DWD fixtures pin convective rain, dry, and both missing sentinels")
    func dwdRasterContract() throws {
        let rain = try fixtureJSON("dwd-rv-convective-rain")
        let dry = try fixtureJSON("dwd-rv-dry-nodata")
        let rainRaster = try #require(rain["raster"] as? [String: Any])
        let dryRaster = try #require(dry["raster"] as? [String: Any])

        #expect(try #require(rainRaster["native_source_resolution_m"] as? Int) == 1_000)
        #expect(try #require(rainRaster["maximum_valid_value"] as? Double) == 5.564)
        #expect(try #require(dryRaster["minimum_valid_value"] as? Double) == -0.001)

        let samples = try #require(dry["sentinel_samples"] as? [[String: Any]])
        let classes = Set(samples.compactMap { $0["classification"] as? String })
        #expect(classes.isSuperset(of: ["nodata", "invalid", "undetect", "zero"]))
    }

    private func fixtureJSON(_ name: String) throws -> [String: Any] {
        let url = try #require(Bundle.module.url(
            forResource: name,
            withExtension: "json",
            subdirectory: "Fixtures"
        ))
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: url))
        return try #require(object as? [String: Any])
    }
}
