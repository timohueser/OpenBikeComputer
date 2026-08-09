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

    @Test("MET non-Nordic fixture proves gust/probability are not worldwide")
    func metWorldwideAvailabilityContract() throws {
        let root = try fixtureJSON("met-locationforecast-manila-24h")
        let availability = try #require(
            root["first_24_availability"] as? [String: Any]
        )
        #expect(try #require(availability["air_temperature_count"] as? Int) == 24)
        #expect(try #require(availability["wind_speed_count"] as? Int) == 24)
        #expect(try #require(availability["precipitation_amount_count"] as? Int) == 24)
        #expect(try #require(availability["symbol_code_count"] as? Int) == 24)
        #expect(try #require(availability["wind_gust_count"] as? Int) == 0)
        #expect(try #require(availability["precipitation_probability_count"] as? Int) == 0)

        let hours = try #require(root["hours"] as? [[String: Any]])
        #expect(hours.count == 24)
        #expect(hours.allSatisfy { $0["wind_gust_mps"] is NSNull })
        #expect(hours.allSatisfy {
            $0["probability_of_precipitation_percent"] is NSNull
        })
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

    @Test("DWD correspondence fixture maps many WCS pixels to unique raw cells")
    func dwdRawWCSCorrespondenceContract() throws {
        let root = try fixtureJSON("dwd-rv-raw-wcs-correspondence")
        let comparison = try #require(root["comparison"] as? [String: Any])
        let valid = try #require(comparison["valid_comparisons"] as? Int)
        #expect(valid == 9_974)
        #expect(try #require(comparison["unique_raw_cells"] as? Int) == valid)
        #expect(try #require(comparison["positive_comparisons"] as? Int) == 10)
        #expect(try #require(comparison["matches_within_1e-6"] as? Int) == valid)
        #expect(
            try #require(comparison["maximum_absolute_error"] as? Double) < 0.000_001
        )

        let samples = try #require(root["samples"] as? [[String: Any]])
        let encodedValues = Set(samples.compactMap { $0["raw_encoded"] as? Int })
        #expect(encodedValues.count >= 10)
        #expect(encodedValues.contains(0))
        #expect(samples.contains {
            ($0["wcs_value"] as? Double) == 4_294_967_296
        })
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
