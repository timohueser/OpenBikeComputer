import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// The production ``WeatherHTTPClient``: the single place Foundation networking enters the weather
/// path. It adds no policy of its own — caching, revalidation and retry are the callers' decisions,
/// because only they know whether a stale answer is usable.
///
/// `URLSession`'s own cache is disabled (`.reloadIgnoringLocalCacheData`) on purpose. The weather
/// path's validators are explicit — the manifest's ETag and MET's `Last-Modified`/`Expires` — and a
/// second, invisible cache underneath them is how a "fresh" fetch quietly returns yesterday's rain.
public struct URLSessionWeatherHTTPClient: WeatherHTTPClient {
    private let session: URLSession
    private let userAgent: String
    private let maximumResponseBytes: Int

    /// - Parameter userAgent: the identifying `User-Agent`. MET's terms require the app to identify
    ///   itself on every request; see ``METLocationforecastAdapter/userAgent(appVersion:)``.
    public init(
        session: URLSession = .shared, userAgent: String,
        maximumResponseBytes: Int = 8 * 1_024 * 1_024
    ) {
        self.session = session
        self.userAgent = userAgent
        self.maximumResponseBytes = maximumResponseBytes
    }

    public func perform(_ request: WeatherHTTPRequest) async throws -> WeatherHTTPResponse {
        var urlRequest = URLRequest(url: request.url)
        urlRequest.httpMethod = "GET"
        urlRequest.cachePolicy = .reloadIgnoringLocalCacheData
        urlRequest.setValue(userAgent, forHTTPHeaderField: "User-Agent")
        urlRequest.setValue("gzip", forHTTPHeaderField: "Accept-Encoding")
        if let range = request.byteRange, !range.isEmpty {
            urlRequest.setValue("bytes=\(range.lowerBound)-\(range.upperBound - 1)",
                                forHTTPHeaderField: "Range")
        }
        if let tag = request.entityTag { urlRequest.setValue(tag, forHTTPHeaderField: "If-None-Match") }
        if let since = request.ifModifiedSince {
            urlRequest.setValue(since, forHTTPHeaderField: "If-Modified-Since")
        }
        for (name, value) in request.headers { urlRequest.setValue(value, forHTTPHeaderField: name) }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: urlRequest)
        } catch {
            throw WeatherHTTPError.transportFailure
        }
        guard let http = response as? HTTPURLResponse else { throw WeatherHTTPError.transportFailure }
        guard data.count <= maximumResponseBytes else { throw WeatherHTTPError.responseTooLarge }

        var headers: [String: String] = [:]
        for (name, value) in http.allHeaderFields {
            if let name = name as? String, let value = value as? String { headers[name] = value }
        }
        return WeatherHTTPResponse(statusCode: http.statusCode, body: data, headers: headers)
    }
}
