import Foundation
import OBCDomain
import OBCWeatherWire

/// MET Norway Locationforecast 2.0 `complete` — the phone's worldwide hourly point forecast, and
/// the **only** third party that ever receives a rider coordinate (epic #1185, WX1).
///
/// Everything MET-shaped stops here: the JSON DTOs are `private`, and the adapter's output is the
/// provider-neutral ``HourlyForecast``. What crosses the edge in the other direction is one rounded
/// coordinate, because the WX1 contract obliges four-decimal rounding — roughly 11 m, far finer than
/// any forecast cell, and coarse enough that the request is not a track log.
///
/// The terms this adapter is built to honour, all from the WX1 decision record:
///
/// - an **identifying `User-Agent`** on every request (MET blocks anonymous traffic);
/// - `Expires` is respected — inside it the cached document is reused with **no** request at all;
/// - past it, revalidation sends `If-Modified-Since` with the exact `Last-Modified` string MET
///   returned, so an unchanged forecast costs a 304 and no body;
/// - concurrent jobs for the same place **coalesce into one request** rather than racing;
/// - a failure keeps the last good document, visibly timestamped, instead of blanking the forecast.
public actor METLocationforecastAdapter: HourlyForecastProvider {
    /// The exact endpoint WX1 pinned. `complete` (not `compact`) because gust and probability of
    /// precipitation only exist there, and both are OBCW fields.
    public static let endpoint = URL(string: "https://api.met.no/weatherapi/locationforecast/2.0/complete")!
    /// Exactly 24 hours reach the device (OBCW §4).
    public static let requiredHours = 24

    /// The identifying `User-Agent` MET requires: product, version and a contact URL.
    public static func userAgent(appVersion: String) -> String {
        "OpenBikeComputer/\(appVersion) github.com/timohueser/OpenBikeComputer"
    }

    private struct CacheEntry: Sendable {
        var forecast: HourlyForecast
        var lastModified: String?
        var expires: Date?
    }

    private let client: any WeatherHTTPClient
    private let endpoint: URL
    private var cache: [String: CacheEntry] = [:]
    private var inFlight: [String: Task<HourlyForecast, any Error>] = [:]

    public init(client: any WeatherHTTPClient, endpoint: URL = METLocationforecastAdapter.endpoint) {
        self.client = client
        self.endpoint = endpoint
    }

    public func hourlyForecast(for request: WeatherRequest, now: Date) async throws -> HourlyForecast {
        guard let position = request.position, position.isValidGeographic else {
            throw WeatherProviderError.noPosition
        }
        let url = Self.url(
            endpoint: endpoint, position: position, altitudeMetres: request.altitudeMetres)
        let key = url.absoluteString

        // Inside `Expires` the answer is already known and MET must not be asked again — the terms
        // call repeated requests inside the stated validity abusive, and it would also be pointless.
        if let entry = cache[key], let expires = entry.expires, now < expires {
            return cached(entry.forecast, now: now)
        }
        // One request per place, however many jobs ask: the second caller awaits the first's task.
        // It needs the *same* cache-on-failure handling as the caller that started that task —
        // otherwise two identical concurrent jobs against a provider that is down give two
        // different answers, one shipping the cached forecast and one failing the whole job. The
        // cache is re-read after the await, since the in-flight attempt may have refreshed it.
        if let existing = inFlight[key] {
            do {
                return try await existing.value
            } catch {
                if let entry = cache[key] { return cached(entry.forecast, now: now) }
                throw error
            }
        }

        let entry = cache[key]
        let task = Task<HourlyForecast, any Error> { [client] in
            let outcome = try await Self.fetch(
                client: client, url: url, validator: entry?.lastModified, now: now)
            switch outcome {
            case let .fresh(fetched):
                self.store(fetched, forKey: key)
                return fetched.forecast
            case let .notModified(expires):
                // MET says the document we hold is still the current one. Reuse it verbatim and
                // adopt the new validity window — a revalidation that threw away the body must not
                // also throw away the answer.
                guard let entry else { throw WeatherProviderError.unavailable }
                self.refresh(key: key, expires: expires)
                return self.cached(entry.forecast, now: now)
            }
        }
        inFlight[key] = task
        defer { inFlight[key] = nil }

        do {
            return try await task.value
        } catch {
            // A provider or network failure is not a blank forecast: the last good document is
            // still true, just older, and it carries its own retrieval timestamp so the UI can say
            // so. Only a cold cache turns the failure into an error the job must handle.
            if let entry { return cached(entry.forecast, now: now) }
            throw error
        }
    }

    private func cached(_ forecast: HourlyForecast, now: Date) -> HourlyForecast {
        var cached = forecast
        cached.isFromCache = true
        return cached
    }

    private func store(_ fetched: Fetched, forKey key: String) {
        cache[key] = CacheEntry(
            forecast: fetched.forecast, lastModified: fetched.lastModified, expires: fetched.expires)
    }

    private func refresh(key: String, expires: Date?) {
        cache[key]?.expires = expires
    }

    // MARK: - Request

    /// Four-decimal rounding, per the WX1 contract. Also what makes the cache key stable: a rider
    /// standing still does not produce a new URL every second.
    static func url(endpoint: URL, position: Coordinate, altitudeMetres: Int?) -> URL {
        var components = URLComponents(url: endpoint, resolvingAgainstBaseURL: false)!
        var items = [
            URLQueryItem(name: "lat", value: fourDecimals(position.latitude)),
            URLQueryItem(name: "lon", value: fourDecimals(position.longitude)),
        ]
        if let altitudeMetres { items.append(URLQueryItem(name: "altitude", value: "\(altitudeMetres)")) }
        components.queryItems = items
        return components.url!
    }

    private static func fourDecimals(_ value: Double) -> String {
        String(format: "%.4f", (value * 10_000).rounded() / 10_000)
    }

    private struct Fetched: Sendable {
        var forecast: HourlyForecast
        var lastModified: String?
        var expires: Date?
    }

    private enum FetchOutcome: Sendable {
        case fresh(Fetched)
        case notModified(expires: Date?)
    }

    private static func fetch(
        client: any WeatherHTTPClient, url: URL, validator: String?, now: Date
    ) async throws -> FetchOutcome {
        let response = try await client.perform(
            WeatherHTTPRequest(url: url, ifModifiedSince: validator))
        if response.isNotModified {
            return .notModified(expires: response.header("Expires").flatMap(HTTPDate.parse))
        }
        if response.statusCode == 429 || response.statusCode == 503 {
            throw WeatherProviderError.rateLimited(retryAfterSeconds: response.retryAfterSeconds)
        }
        guard response.isSuccess else { throw WeatherProviderError.unavailable }

        let hours = try decode(response.body, now: now)
        let lastModified = response.header("Last-Modified")
        return .fresh(Fetched(
            forecast: HourlyForecast(
                hours: hours, attribution: .met, retrievedAt: now,
                providerUpdatedAt: lastModified.flatMap(HTTPDate.parse)),
            lastModified: lastModified,
            expires: response.header("Expires").flatMap(HTTPDate.parse)))
    }

    // MARK: - Decode

    /// Decode exactly 24 consecutive hourly records, or refuse.
    ///
    /// "Refuse" is the important half. A record whose optional keys are absent is fine — Manila
    /// supplied neither gust nor probability in any of WX1's 24 captured hours — but a key that is
    /// present and is a string, an object, `null` or out of range is **malformed**, not
    /// "unavailable". Silently degrading a broken payload into missing values would let a provider
    /// change nobody noticed become a forecast nobody can trust.
    static func decode(_ body: Data, now: Date) throws -> [HourlyCondition] {
        let document: Document
        do {
            document = try JSONDecoder().decode(Document.self, from: body)
        } catch {
            throw WeatherProviderError.malformedResponse
        }
        let units = document.properties.meta.units
        guard units.air_temperature == "celsius", units.wind_speed == "m/s",
              units.wind_from_direction == "degrees", units.precipitation_amount == "mm"
        else { throw WeatherProviderError.malformedResponse }
        if let gust = units.wind_speed_of_gust, gust != "m/s" {
            throw WeatherProviderError.malformedResponse
        }
        if let probability = units.probability_of_precipitation, probability != "%" {
            throw WeatherProviderError.malformedResponse
        }

        let series = document.properties.timeseries.prefix(requiredHours)
        guard series.count == requiredHours else { throw WeatherProviderError.malformedResponse }

        var hours: [HourlyCondition] = []
        hours.reserveCapacity(requiredHours)
        for entry in series {
            guard let time = RFC3339.parse(entry.time) else {
                throw WeatherProviderError.malformedResponse
            }
            // Canonical UTC seconds, exactly one hour apart — the contract, and the reason the OBCW
            // hourly section can be a fixed 24 x 3600 lattice with no per-record timestamps.
            if let previous = hours.last {
                guard time.timeIntervalSince(previous.validAt) == 3_600 else {
                    throw WeatherProviderError.malformedResponse
                }
            }
            let instant = entry.data.instant.details
            let next = entry.data.next_1_hours
            guard let symbol = next?.summary.symbol_code,
                  let condition = METSymbolMapping.condition(for: symbol)
            else { throw WeatherProviderError.malformedResponse }
            guard let temperature = instant.air_temperature, temperature.isFinite,
                  let windFrom = instant.wind_from_direction, windFrom.isFinite,
                  let windSpeed = instant.wind_speed, windSpeed.isFinite, windSpeed >= 0,
                  let amount = next?.details?.precipitation_amount, amount.isFinite, amount >= 0
            else { throw WeatherProviderError.malformedResponse }
            if let gust = instant.wind_speed_of_gust, !gust.isFinite || gust < 0 {
                throw WeatherProviderError.malformedResponse
            }
            if let probability = next?.details?.probability_of_precipitation,
               !probability.isFinite || probability < 0 || probability > 100 {
                throw WeatherProviderError.malformedResponse
            }
            hours.append(HourlyCondition(
                validAt: time,
                temperatureCelsius: temperature,
                precipitationMillimetres: amount,
                precipitationProbabilityPercent: next?.details?.probability_of_precipitation,
                condition: condition,
                windFromDegrees: windFrom,
                windSpeedMetresPerSecond: windSpeed,
                windGustMetresPerSecond: instant.wind_speed_of_gust))
        }
        return hours
    }

    // The provider's own shape, and the only place it exists. Optional-typed throughout so an
    // absent key decodes; presence-with-wrong-type still fails, which is what we want.
    private struct Document: Decodable {
        var properties: Properties
    }

    private struct Properties: Decodable {
        var meta: Meta
        var timeseries: [Series]
    }

    private struct Meta: Decodable {
        var units: Units
    }

    private struct Units: Decodable {
        var air_temperature: String?
        var precipitation_amount: String?
        var probability_of_precipitation: String?
        var wind_from_direction: String?
        var wind_speed: String?
        var wind_speed_of_gust: String?
    }

    private struct Series: Decodable {
        var time: String
        var data: SeriesData
    }

    private struct SeriesData: Decodable {
        var instant: Instant
        var next_1_hours: NextHour?
    }

    private struct Instant: Decodable {
        var details: InstantDetails
    }

    private struct InstantDetails: Decodable {
        var air_temperature: Double?
        var wind_from_direction: Double?
        var wind_speed: Double?
        var wind_speed_of_gust: Double?
    }

    private struct NextHour: Decodable {
        var summary: NextHourSummary
        var details: NextHourDetails?
    }

    private struct NextHourSummary: Decodable {
        var symbol_code: String?
    }

    private struct NextHourDetails: Decodable {
        var precipitation_amount: Double?
        var probability_of_precipitation: Double?
    }
}
