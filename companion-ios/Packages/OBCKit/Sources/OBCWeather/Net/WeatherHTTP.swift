import Foundation

/// One HTTP request the weather path wants performed.
///
/// A value type rather than a `URLRequest` for the same reason the app talks to `DeviceTransport`
/// instead of CoreBluetooth: the whole weather domain is then drivable from fixtures, on a host,
/// with no URL loading system in the process. `URLSessionWeatherHTTPClient` is the one place that
/// turns this into Foundation networking.
public struct WeatherHTTPRequest: Equatable, Sendable {
    public var url: URL
    /// Inclusive-start, exclusive-end byte range, sent as `Range: bytes=start-(end-1)`.
    public var byteRange: Range<Int>?
    /// `If-None-Match` — the manifest's revalidation validator.
    public var entityTag: String?
    /// `If-Modified-Since` — MET's required revalidation validator, sent as the exact
    /// `Last-Modified` string the provider gave us, never a reformatted date.
    public var ifModifiedSince: String?
    public var headers: [String: String]

    public init(
        url: URL, byteRange: Range<Int>? = nil, entityTag: String? = nil,
        ifModifiedSince: String? = nil, headers: [String: String] = [:]
    ) {
        self.url = url
        self.byteRange = byteRange
        self.entityTag = entityTag
        self.ifModifiedSince = ifModifiedSince
        self.headers = headers
    }
}

public struct WeatherHTTPResponse: Equatable, Sendable {
    public var statusCode: Int
    public var body: Data
    /// Response headers, matched case-insensitively through ``header(_:)``.
    public var headers: [String: String]

    public init(statusCode: Int, body: Data, headers: [String: String] = [:]) {
        self.statusCode = statusCode
        self.body = body
        self.headers = headers
    }

    public func header(_ name: String) -> String? {
        headers.first { $0.key.caseInsensitiveCompare(name) == .orderedSame }?.value
    }

    public var isNotModified: Bool { statusCode == 304 }
    public var isSuccess: Bool { (200..<300).contains(statusCode) }
}

/// The injectable networking seam. `Sendable` because weather jobs run concurrently and every
/// implementation must be safe to share.
public protocol WeatherHTTPClient: Sendable {
    func perform(_ request: WeatherHTTPRequest) async throws -> WeatherHTTPResponse
}

public enum WeatherHTTPError: Error, Equatable, Sendable {
    /// The transport failed outright — offline, DNS, TLS, timeout.
    case transportFailure
    /// A status this caller cannot use. `retryAfterSeconds` carries a `Retry-After` when the
    /// provider sent one, so a rate limit is respected rather than hammered.
    case unacceptableStatus(code: Int, retryAfterSeconds: Int?)
    /// A Range request came back without the bytes that were asked for. Never silently accepted:
    /// a server that answers `200 OK` with the whole object to a Range request would otherwise
    /// have its body parsed as if it started at the requested offset.
    case rangeNotHonoured
    case responseTooLarge
}

public extension WeatherHTTPResponse {
    /// `Retry-After` in seconds, whether the provider expressed it as a delay or an HTTP date.
    var retryAfterSeconds: Int? {
        guard let value = header("Retry-After") else { return nil }
        if let seconds = Int(value.trimmingCharacters(in: .whitespaces)) { return seconds }
        guard let date = HTTPDate.parse(value) else { return nil }
        return Swift.max(0, Int(date.timeIntervalSinceNow.rounded()))
    }
}

/// RFC 7231 HTTP-date parsing/formatting, in the fixed `en_US_POSIX`/GMT locale HTTP requires.
///
/// `Expires` and `Last-Modified` are the two MET headers the WX1 contract obliges us to honour, and
/// a device locale must never change how they read.
public enum HTTPDate {
    // Configured once, never mutated. `DateFormatter` is documented thread-safe for formatting and
    // parsing; `RFC3339`'s `ISO8601DateFormatter`s below need `nonisolated(unsafe)` to say the same
    // thing because that type predates `Sendable` auditing. Building a formatter per timestamp
    // instead would allocate several for every frame in every manifest.
    private static let formatters: [DateFormatter] = {
        ["EEE, dd MMM yyyy HH:mm:ss zzz", "EEEE, dd-MMM-yy HH:mm:ss zzz", "EEE MMM d HH:mm:ss yyyy"]
            .map { format in
                let formatter = DateFormatter()
                formatter.locale = Locale(identifier: "en_US_POSIX")
                formatter.timeZone = TimeZone(secondsFromGMT: 0)
                formatter.dateFormat = format
                return formatter
            }
    }()

    public static func parse(_ value: String) -> Date? {
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        for formatter in formatters {
            if let date = formatter.date(from: trimmed) { return date }
        }
        return nil
    }

    public static func string(from date: Date) -> String {
        formatters[0].string(from: date)
    }
}

/// RFC 3339 seconds — the one timestamp form the frozen manifest schema uses.
public enum RFC3339 {
    nonisolated(unsafe) private static let formatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        return formatter
    }()

    nonisolated(unsafe) private static let fractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        return formatter
    }()

    /// **The date-time separator must be `T`.**
    ///
    /// RFC 3339 §5.6 lets an application accept a space there, and chrono — the Rust client's
    /// parser — does; `ISO8601DateFormatter` does not. Left implicit, that is a rejection
    /// divergence between the two clients over the one document every rider fetches first, so it is
    /// stated rather than inherited: both sides now require `T` (or `t`) at index 10, and the
    /// shared `rejection_equivalence` corpus pins it. The baker writes `T`, so nothing legitimate is
    /// refused.
    public static func parse(_ value: String) -> Date? {
        let bytes = Array(value.utf8)
        guard bytes.count > 10, bytes[10] == UInt8(ascii: "T") || bytes[10] == UInt8(ascii: "t")
        else { return nil }
        return formatter.date(from: value) ?? fractional.date(from: value)
    }

    public static func string(from date: Date) -> String { formatter.string(from: date) }
}
