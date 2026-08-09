import Foundation
@testable import OBCWeather

/// A fixture-driven ``WeatherHTTPClient`` with a full request ledger.
///
/// The ledger is the point: OBCG's corridor contract is a statement about *which bytes* a client
/// fetches, so the tests assert on the exact ranges rather than on the decoded result alone. A
/// client that downloaded whole frames would still decode the right cells.
final class StubWeatherHTTPClient: WeatherHTTPClient, @unchecked Sendable {
    struct Object {
        var bytes: Data
        var headers: [String: String] = [:]
        /// Forced status; `nil` serves 200/206 from `bytes`.
        var status: Int?
        /// When set, a request carrying this `If-None-Match` gets a 304.
        var entityTag: String?
        /// When set, a request carrying this `If-Modified-Since` gets a 304.
        var lastModified: String?
        /// Answer Range requests with the whole object and a 200 — a legal but unhelpful server.
        var ignoresRange = false
        /// Fail the transport outright.
        var offline = false
    }

    private let lock = NSLock()
    private var objects: [String: Object]
    private var ledger: [WeatherHTTPRequest] = []
    /// Set to slow a response down so concurrency tests can actually overlap.
    var delayNanoseconds: UInt64 = 0

    init(objects: [String: Object] = [:]) {
        self.objects = objects
    }

    var requests: [WeatherHTTPRequest] {
        lock.withLock { ledger }
    }

    func requests(forPathSuffix suffix: String) -> [WeatherHTTPRequest] {
        requests.filter { $0.url.path.hasSuffix(suffix) }
    }

    func set(_ object: Object, for path: String) {
        lock.withLock { objects[path] = object }
    }

    func mutate(_ path: String, _ change: (inout Object) -> Void) {
        lock.withLock {
            guard var object = objects[path] else { return }
            change(&object)
            objects[path] = object
        }
    }

    func resetLedger() {
        lock.withLock { ledger = [] }
    }

    func perform(_ request: WeatherHTTPRequest) async throws -> WeatherHTTPResponse {
        if delayNanoseconds > 0 { try? await Task.sleep(nanoseconds: delayNanoseconds) }
        let object = lock.withLock { () -> Object? in
            ledger.append(request)
            return objects.first { request.url.path.hasSuffix($0.key) }?.value
        }

        guard let object else { return WeatherHTTPResponse(statusCode: 404, body: Data()) }
        if object.offline { throw WeatherHTTPError.transportFailure }
        if let status = object.status, status != 200 {
            return WeatherHTTPResponse(statusCode: status, body: Data(), headers: object.headers)
        }
        if let tag = object.entityTag, request.entityTag == tag {
            return WeatherHTTPResponse(statusCode: 304, body: Data(), headers: object.headers)
        }
        if let modified = object.lastModified, request.ifModifiedSince == modified {
            return WeatherHTTPResponse(statusCode: 304, body: Data(), headers: object.headers)
        }
        guard let range = request.byteRange, !object.ignoresRange else {
            return WeatherHTTPResponse(
                statusCode: 200, body: object.bytes, headers: object.headers)
        }
        guard range.lowerBound >= 0, range.upperBound <= object.bytes.count else {
            return WeatherHTTPResponse(statusCode: 416, body: Data(), headers: object.headers)
        }
        let slice = Data(object.bytes[range.lowerBound..<range.upperBound])
        return WeatherHTTPResponse(statusCode: 206, body: slice, headers: object.headers)
    }
}
