import Foundation
import OBCDomain

/// A place shared from a map app, decoded from the share-sheet text.
public enum SharedPlace: Equatable, Sendable {
    /// A position the share names directly.
    case coordinate(Coordinate, name: String?)
    /// A short link that only a redirect turns into a position.
    case shortLink(URL, name: String?)
    /// Words for the planner search, such as a name or an address.
    case query(String)

    /// Nil when the text holds nothing that looks like a place.
    /// The first map link wins and takes the first text line as its fallback name; text without a map
    /// link becomes a query of its lines joined by ", ".
    public init?(text: String) {
        var urls: [URL] = []
        var lines: [String] = []
        for line in text.split(whereSeparator: \.isNewline) {
            var words: [Substring] = []
            for token in line.split(whereSeparator: \.isWhitespace) {
                if let url = URL(string: String(token)), url.host() != nil,
                   ["http", "https"].contains(url.scheme?.lowercased() ?? "") {
                    urls.append(url)
                } else {
                    words.append(token)
                }
            }
            if !words.isEmpty { lines.append(words.joined(separator: " ")) }
        }
        if let place = urls.lazy.compactMap({ Self.place(from: $0, name: lines.first) }).first {
            self = place
        } else if lines.isEmpty {
            return nil
        } else {
            self = .query(lines.joined(separator: ", "))
        }
    }

    /// The name to show to the rider, if the share has one.
    public var name: String? {
        switch self {
        case .coordinate(_, let name), .shortLink(_, let name): name
        case .query(let text): text
        }
    }

    /// `fallback` names the place only when the link itself carries no name.
    fileprivate static func place(from url: URL, name fallback: String?) -> SharedPlace? {
        guard let host = url.host()?.lowercased() else { return nil }
        let path = url.path(percentEncoded: true)
        let query = queryItems(url)
        if host == "maps.app.goo.gl" || host == "maps.apple" || (host == "goo.gl" && path.hasPrefix("/maps")) {
            return .shortLink(url, name: fallback)
        }
        // Google sends EU visitors here and keeps the map link in `continue`.
        if host == "consent.google.com" {
            return query["continue"].flatMap { URL(string: $0) }.flatMap { place(from: $0, name: fallback) }
        }
        if host == "maps.apple.com" {
            if let position = coordinate(query["coordinate"] ?? query["ll"]) {
                return .coordinate(position, name: query["name"] ?? query["q"] ?? fallback)
            }
            return (query["q"] ?? query["address"]).map(SharedPlace.query)
        }
        let labels = host.split(separator: ".")
        guard labels.contains("google"), labels.first == "maps" || path.hasPrefix("/maps") else { return nil }
        let segments = path.split(separator: "/")
        let name = segments.firstIndex(of: "place").flatMap { $0 + 1 < segments.endIndex ? decode(segments[$0 + 1]) : nil }
        let text = query["q"] ?? query["query"]
        // `!3d…!4d…` is the place itself; `@lat,lon` is only the map viewport.
        let pinned = path.firstMatch(of: #/!3d(-?\d+(?:\.\d+)?)!4d(-?\d+(?:\.\d+)?)/#).flatMap { coordinate($0.1, $0.2) }
        let viewport = path.firstMatch(of: #/@(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)/#).flatMap { coordinate($0.1, $0.2) }
        if let position = pinned ?? coordinate(text) ?? coordinate(query["ll"]) ?? viewport {
            return .coordinate(position, name: name ?? fallback)
        }
        return (text ?? name).map(SharedPlace.query)
    }

    private static func coordinate(_ pair: String?) -> Coordinate? {
        guard let parts = pair?.split(separator: ","), parts.count == 2 else { return nil }
        return coordinate(parts[0], parts[1])
    }

    private static func coordinate(_ latitude: Substring, _ longitude: Substring) -> Coordinate? {
        guard let latitude = Double(latitude.trimmingCharacters(in: .whitespaces)),
              let longitude = Double(longitude.trimmingCharacters(in: .whitespaces)) else { return nil }
        let position = Coordinate(latitude: latitude, longitude: longitude)
        return position.isValidGeographic ? position : nil
    }

    /// Form decoding: `+` is a space, so it is replaced before `%2B` turns into a literal plus.
    private static func decode(_ raw: Substring) -> String? {
        let text = raw.replacingOccurrences(of: "+", with: " ").removingPercentEncoding?
            .trimmingCharacters(in: .whitespaces)
        return text?.isEmpty == false ? text : nil
    }

    private static func queryItems(_ url: URL) -> [String: String] {
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.percentEncodedQueryItems ?? []
        return items.reduce(into: [:]) { result, item in
            if result[item.name] == nil, let value = item.value.flatMap({ decode(Substring($0)) }) {
                result[item.name] = value
            }
        }
    }
}

/// A short link whose redirects do not reach a place.
public struct UnresolvedSharedLink: Error, Equatable, Sendable {
    public let url: URL
}

/// Expands short links by reading redirect `Location` headers one hop at a time, never fetching the
/// final page (Google sends EU visitors to a consent page).
public struct SharedPlaceResolver: Sendable {
    private let session: URLSession

    public init(session: URLSession = .shared) {
        self.session = session
    }

    /// Returns `.coordinate` or `.query`; a `.shortLink` is expanded, other cases return unchanged.
    /// Throws `UnresolvedSharedLink` when five hops do not reach a place.
    public func resolve(_ place: SharedPlace) async throws -> SharedPlace {
        guard case .shortLink(var url, let name) = place else { return place }
        for _ in 0..<5 {
            let (_, response) = try await session.data(for: URLRequest(url: url), delegate: NoRedirects())
            guard let location = (response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Location"),
                  let next = URL(string: location, relativeTo: url)?.absoluteURL else { break }
            switch SharedPlace.place(from: next, name: name) {
            case .shortLink?, nil: url = next
            case let expanded?: return expanded
            }
        }
        throw UnresolvedSharedLink(url: url)
    }
}

private final class NoRedirects: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest) async -> URLRequest? {
        nil
    }
}
