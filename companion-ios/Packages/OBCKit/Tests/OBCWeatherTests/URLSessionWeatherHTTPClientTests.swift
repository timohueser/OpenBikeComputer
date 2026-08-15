import Foundation
import Testing
@testable import OBCWeather

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// The production client, driven through a `URLProtocol` fixture.
///
/// Everything above it is tested against the ``WeatherHTTPClient`` seam, so this is the one place
/// that proves the *real* request actually carries what the two providers' terms require: the
/// identifying `User-Agent` MET obliges, the `Range` header the OBCG corridor contract is built on,
/// and the validators that keep both from being re-downloaded.
struct URLSessionWeatherHTTPClientTests {
    final class RecordingProtocol: URLProtocol, @unchecked Sendable {
        nonisolated(unsafe) static var lastRequest: URLRequest?
        nonisolated(unsafe) static var responseHeaders: [String: String] = ["ETag": "\"v1\""]
        nonisolated(unsafe) static var statusCode = 206
        nonisolated(unsafe) static var body = Data("payload".utf8)

        override class func canInit(with request: URLRequest) -> Bool { true }
        override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

        override func startLoading() {
            Self.lastRequest = request
            let response = HTTPURLResponse(
                url: request.url!, statusCode: Self.statusCode, httpVersion: "HTTP/1.1",
                headerFields: Self.responseHeaders)!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: Self.body)
            client?.urlProtocolDidFinishLoading(self)
        }

        override func stopLoading() {}
    }

    static func client() -> URLSessionWeatherHTTPClient {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [RecordingProtocol.self]
        return URLSessionWeatherHTTPClient(
            session: URLSession(configuration: configuration),
            userAgent: METLocationforecastAdapter.userAgent(appVersion: "0.1"))
    }

    @Test
    func theRequestCarriesTheIdentifyingAgentTheRangeAndTheValidators() async throws {
        let response = try await Self.client().perform(WeatherHTTPRequest(
            url: URL(string: "https://wx.example.invalid/wx/v2/20260810T1430Z/f0/s0-0.obcg")!,
            byteRange: 128..<256, entityTag: "\"v1\"",
            ifModifiedSince: "Sun, 09 Aug 2026 08:27:39 GMT"))

        let request = try #require(RecordingProtocol.lastRequest)
        #expect(request.value(forHTTPHeaderField: "User-Agent")
            == "OpenBikeComputer/0.1 github.com/timohueser/OpenBikeComputer")
        // Inclusive-inclusive on the wire, half-open in Swift: 128..<256 is `bytes=128-255`.
        #expect(request.value(forHTTPHeaderField: "Range") == "bytes=128-255")
        #expect(request.value(forHTTPHeaderField: "If-None-Match") == "\"v1\"")
        #expect(request.value(forHTTPHeaderField: "If-Modified-Since")
            == "Sun, 09 Aug 2026 08:27:39 GMT")
        #expect(request.value(forHTTPHeaderField: "Accept-Encoding") == "gzip")
        // The URL loading system's own cache is bypassed: this path's validators are explicit, and
        // a second invisible cache underneath them is how a "fresh" fetch returns old rain.
        #expect(request.cachePolicy == .reloadIgnoringLocalCacheData)

        #expect(response.statusCode == 206)
        #expect(response.header("etag") == "\"v1\"", "headers match case-insensitively")
    }

    @Test
    func aTransportFailureIsReportedAsOneRatherThanLeakingURLError() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = []
        let client = URLSessionWeatherHTTPClient(
            session: URLSession(configuration: configuration), userAgent: "test")
        await #expect(throws: WeatherHTTPError.transportFailure) {
            try await client.perform(WeatherHTTPRequest(
                url: URL(string: "https://localhost:1/never")!))
        }
    }
}
