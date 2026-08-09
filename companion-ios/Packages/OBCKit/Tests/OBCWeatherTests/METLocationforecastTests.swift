import Foundation
import OBCDomain
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// The MET adapter against the WX1 captures.
///
/// The values are the ones WX1 actually recorded from `api.met.no` on 2026-08-09 — Oslo, which
/// supplied gust and probability in all 24 hours, and Manila, which supplied neither in any. That
/// pair is the whole reason the domain's optionals are optional.
struct METLocationforecastTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    static let osloURL = "/weatherapi/locationforecast/2.0/complete"

    static func adapter(
        _ capture: WeatherFixtures.METCapture, headers: [String: String] = [:],
        unitOverrides: [String: String] = [:]
    ) -> (METLocationforecastAdapter, StubWeatherHTTPClient) {
        let http = StubWeatherHTTPClient(objects: [
            osloURL: StubWeatherHTTPClient.Object(
                bytes: capture.locationforecastJSON(unitOverrides: unitOverrides),
                headers: headers,
                lastModified: headers["Last-Modified"]),
        ])
        return (METLocationforecastAdapter(client: http), http)
    }

    static func request(_ capture: WeatherFixtures.METCapture) -> WeatherRequest {
        WeatherRequest(
            requestID: 7,
            position: Coordinate(
                latitude: capture.provenance.latitude, longitude: capture.provenance.longitude),
            fixTime: now, altitudeMetres: capture.provenance.altitude_m)
    }

    // MARK: - Mapping

    @Test
    func aWorldwideCoordinateYields24HoursWithMETAttribution() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (adapter, http) = Self.adapter(capture)
        let forecast = try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)

        #expect(forecast.hours.count == 24)
        #expect(forecast.attribution == .met)
        #expect(forecast.attribution.text == "Data from MET Norway")
        #expect(!forecast.isFromCache)

        let first = try #require(forecast.hours.first)
        #expect(first.temperatureCelsius == 17.4)
        #expect(first.windFromDegrees == 189.0)
        #expect(first.windSpeedMetresPerSecond == 5.5)
        #expect(first.windGustMetresPerSecond == 9.8)
        #expect(first.precipitationProbabilityPercent == 1.4)
        #expect(first.precipitationMillimetres == 0.0)
        #expect(first.condition == .overcast, "MET `cloudy` is the canonical overcast")
        // Consecutive whole hours, which is what lets OBCW carry a fixed 24 x 3600 lattice.
        for index in 1..<forecast.hours.count {
            #expect(forecast.hours[index].validAt
                .timeIntervalSince(forecast.hours[index - 1].validAt) == 3_600)
        }
        // One request, and it identifies the app and rounds the coordinate.
        let url = try #require(http.requests.first?.url)
        #expect(url.query?.contains("lat=59.9139") == true)
        #expect(url.query?.contains("lon=10.7522") == true)
        #expect(url.query?.contains("altitude=23") == true)
    }

    /// Manila supplied neither gust nor probability. Those stay `nil` — never zero, which would
    /// read as "no chance of rain" on a device that cannot tell the difference.
    @Test
    func absentOptionalFieldsStayUnavailableRatherThanBecomingZero() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-manila-24h.json")
        let (adapter, _) = Self.adapter(capture)
        let forecast = try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)
        #expect(forecast.hours.count == 24)
        #expect(forecast.hours.allSatisfy { $0.windGustMetresPerSecond == nil })
        #expect(forecast.hours.allSatisfy { $0.precipitationProbabilityPercent == nil })
        #expect(forecast.hours.allSatisfy { $0.temperatureCelsius != nil })
    }

    @Test
    func fourDecimalRoundingIsAppliedToTheRequestCoordinate() {
        let url = METLocationforecastAdapter.url(
            endpoint: METLocationforecastAdapter.endpoint,
            position: Coordinate(latitude: 47.123456789, longitude: -7.987654321),
            altitudeMetres: nil)
        #expect(url.query == "lat=47.1235&lon=-7.9877")
        #expect(url.absoluteString.hasPrefix(
            "https://api.met.no/weatherapi/locationforecast/2.0/complete?"))
    }

    @Test
    func theUserAgentIdentifiesTheApp() {
        #expect(METLocationforecastAdapter.userAgent(appVersion: "1.2.3")
            == "OpenBikeComputer/1.2.3 github.com/timohueser/OpenBikeComputer")
    }

    @Test(arguments: [
        ("clearsky_day", OBCWeatherCondition.clear),
        ("clearsky_polartwilight", .clear),
        ("fair_night", .mostlyClear),
        ("partlycloudy_day", .partlyCloudy),
        ("cloudy", .overcast),
        ("fog", .fog),
        ("lightrain", .drizzle),
        ("rain", .rain),
        ("heavyrain", .rain),
        ("rainshowers_day", .showers),
        ("lightrainshowers_night", .showers),
        ("sleet", .sleet),
        ("heavysleetshowers_day", .sleet),
        ("snow", .snow),
        ("lightsnowshowers_polartwilight", .snow),
        ("rainandthunder", .thunderstorm),
        ("heavysleetshowersandthunder_day", .thunderstorm),
        ("lightssnowshowersandthunder_night", .thunderstorm),
        ("somethingnobodyhasinvented", .unavailable),
    ])
    func theFrozenSymbolMappingHolds(code: String, expected: OBCWeatherCondition) {
        #expect(METSymbolMapping.condition(for: code) == expected)
    }

    @Test
    func anEmptySymbolIsMalformedRatherThanUnknown() {
        #expect(METSymbolMapping.condition(for: "") == nil)
        #expect(METSymbolMapping.condition(for: "   ") == nil)
        #expect(METSymbolMapping.condition(for: "_day") == nil)
    }

    // MARK: - Refusals

    @Test
    func advertisedUnitsAreValidated() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (adapter, _) = Self.adapter(capture, unitOverrides: ["air_temperature": "fahrenheit"])
        await #expect(throws: WeatherProviderError.malformedResponse) {
            try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)
        }
    }

    @Test
    func aShortOrMisalignedSeriesIsRefused() throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        var document = try JSONSerialization.jsonObject(
            with: capture.locationforecastJSON()) as! [String: Any]
        var properties = document["properties"] as! [String: Any]
        var series = properties["timeseries"] as! [[String: Any]]

        properties["timeseries"] = Array(series.prefix(23))
        document["properties"] = properties
        #expect(throws: WeatherProviderError.malformedResponse) {
            try METLocationforecastAdapter.decode(
                try JSONSerialization.data(withJSONObject: document), now: Self.now)
        }

        // A half-hour step is not an hourly forecast, whatever it claims to be.
        series[3]["time"] = "2026-08-09T11:30:00Z"
        properties["timeseries"] = series
        document["properties"] = properties
        #expect(throws: WeatherProviderError.malformedResponse) {
            try METLocationforecastAdapter.decode(
                try JSONSerialization.data(withJSONObject: document), now: Self.now)
        }
    }

    /// A present-but-wrong value is malformed, not "unavailable". Quietly degrading it would turn a
    /// provider change nobody noticed into a forecast nobody can trust.
    @Test
    func aPresentButInvalidValueIsMalformed() throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        for mutation in ["probability", "gust", "symbol", "temperature"] {
            var document = try JSONSerialization.jsonObject(
                with: capture.locationforecastJSON()) as! [String: Any]
            var properties = document["properties"] as! [String: Any]
            var series = properties["timeseries"] as! [[String: Any]]
            var data = series[0]["data"] as! [String: Any]
            var instant = data["instant"] as! [String: Any]
            var instantDetails = instant["details"] as! [String: Any]
            var next = data["next_1_hours"] as! [String: Any]
            var nextDetails = next["details"] as! [String: Any]
            switch mutation {
            case "probability": nextDetails["probability_of_precipitation"] = 140
            case "gust": instantDetails["wind_speed_of_gust"] = -1
            case "symbol": next["summary"] = ["symbol_code": ""]
            default: instantDetails["air_temperature"] = "17.4"
            }
            instant["details"] = instantDetails
            next["details"] = nextDetails
            data["instant"] = instant
            data["next_1_hours"] = next
            series[0]["data"] = data
            properties["timeseries"] = series
            document["properties"] = properties
            #expect(throws: WeatherProviderError.malformedResponse, "\(mutation) accepted") {
                try METLocationforecastAdapter.decode(
                    try JSONSerialization.data(withJSONObject: document), now: Self.now)
            }
        }
    }

    // MARK: - Caching and request discipline

    @Test
    func aCachedForecastIsReusedInsideExpiresWithNoSecondRequest() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let expires = HTTPDate.string(from: Self.now.addingTimeInterval(1_800))
        let (adapter, http) = Self.adapter(capture, headers: ["Expires": expires])
        _ = try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)
        let second = try await adapter.hourlyForecast(
            for: Self.request(capture), now: Self.now.addingTimeInterval(300))
        #expect(http.requests.count == 1)
        #expect(second.isFromCache)
    }

    @Test
    func revalidationSendsTheExactLastModifiedAndA304KeepsTheDocument() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let lastModified = HTTPDate.string(from: Self.now.addingTimeInterval(-600))
        let (adapter, http) = Self.adapter(
            capture, headers: ["Last-Modified": lastModified,
                               "Expires": HTTPDate.string(from: Self.now)])
        let first = try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)
        let second = try await adapter.hourlyForecast(
            for: Self.request(capture), now: Self.now.addingTimeInterval(3_600))
        #expect(http.requests.count == 2)
        #expect(http.requests[0].ifModifiedSince == nil)
        #expect(http.requests[1].ifModifiedSince == lastModified)
        #expect(second.isFromCache)
        #expect(second.hours == first.hours)
    }

    @Test
    func aRateLimitFallsBackToTheCacheAndNeverRetriesThrough() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (adapter, http) = Self.adapter(capture)
        let fresh = try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)
        http.mutate(Self.osloURL) { $0.status = 429; $0.headers["Retry-After"] = "120" }
        let cached = try await adapter.hourlyForecast(
            for: Self.request(capture), now: Self.now.addingTimeInterval(3_600))
        #expect(cached.hours == fresh.hours)
        #expect(cached.isFromCache)
        #expect(cached.retrievedAt == Self.now, "a cached forecast keeps its true retrieval time")
        #expect(http.requests.count == 2, "one attempt, not a retry storm")
    }

    @Test
    func offlineWithNoCacheIsAnHonestFailure() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (adapter, http) = Self.adapter(capture)
        http.mutate(Self.osloURL) { $0.offline = true }
        await #expect(throws: (any Error).self) {
            try await adapter.hourlyForecast(for: Self.request(capture), now: Self.now)
        }
    }

    @Test
    func aRequestWithNoPositionNeverReachesTheProvider() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (adapter, http) = Self.adapter(capture)
        await #expect(throws: WeatherProviderError.noPosition) {
            try await adapter.hourlyForecast(for: WeatherRequest(requestID: 1), now: Self.now)
        }
        #expect(http.requests.isEmpty)
    }

    /// Concurrent weather jobs for the same place must cost MET one request, not two.
    @Test
    func concurrentJobsForOnePlaceCoalesceIntoOneRequest() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (adapter, http) = Self.adapter(capture)
        http.delayNanoseconds = 20_000_000
        let request = Self.request(capture)
        async let first = adapter.hourlyForecast(for: request, now: Self.now)
        async let second = adapter.hourlyForecast(for: request, now: Self.now)
        let forecasts = try await [first, second]
        #expect(http.requests.count == 1)
        #expect(forecasts[0].hours == forecasts[1].hours)
    }
}
