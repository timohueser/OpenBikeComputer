import Foundation

public struct PlaceContent: Decodable, Equatable, Sendable {
    public struct Attribution: Decodable, Equatable, Sendable {
        public let source_url: String
        public let revision: String
        public let license_url: String
    }
    public struct Variant: Decodable, Equatable, Sendable {
        public let language: String
        public let text_pages: [String]
        public let attribution: Attribution
    }
    public struct FileIdentity: Decodable, Hashable, Sendable {
        public let filename: String
        public let page_id: UInt64
    }
    public struct FileRevision: Decodable, Hashable, Sendable {
        public let timestamp: String
        public let sha1: String
    }
    public struct Photo: Decodable, Hashable, Sendable {
        public let source_url: String
        public let revision: String
        public let license_url: String
        public let online_url: String?
        public let credit: [String]
        public let file_identity: FileIdentity?
        public let page_revision: UInt64?
        public let file_revision: FileRevision?

        public func onlineURL(session: URLSession = .shared) async throws -> URL? {
            guard let online_url, let file_identity, let page_revision, let file_revision else { return nil }
            var components = URLComponents(string: "https://commons.wikimedia.org/w/api.php")!
            components.queryItems = ["action": "query", "format": "json", "formatversion": "2", "prop": "info|imageinfo",
                "pageids": String(file_identity.page_id), "iiprop": "timestamp|sha1|url", "iiurlwidth": "500", "iilimit": "1",
                "maxage": "0", "smaxage": "0", "maxlag": "5"].sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
            var request = URLRequest(url: components.url!, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 10)
            request.setValue("OpenBikeComputer planner (https://openbikecomputer.com)", forHTTPHeaderField: "User-Agent")
            let (data, response) = try await session.data(for: request)
            try Task.checkCancellation()
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
            struct Reply: Decodable {
                struct Query: Decodable {
                    struct Page: Decodable {
                        struct Image: Decodable { let timestamp: String; let sha1: String; let thumburl: String? }
                        let pageid: UInt64?
                        let lastrevid: UInt64?
                        let title: String
                        let imageinfo: [Image]?
                    }
                    let pages: [Page]
                }
                let query: Query?
            }
            let reply = try JSONDecoder().decode(Reply.self, from: data)
            let filename = file_identity.filename.hasPrefix("File:") ? String(file_identity.filename.dropFirst(5)) : file_identity.filename
            guard let page = reply.query?.pages.first, page.pageid == file_identity.page_id, page.lastrevid == page_revision,
                  page.title == "File:\(filename.replacingOccurrences(of: "_", with: " "))",
                  let image = page.imageinfo?.first, image.timestamp == file_revision.timestamp, image.sha1 == file_revision.sha1,
                  image.thumburl == online_url, let url = URL(string: online_url), url.scheme == "https",
                  url.host == "upload.wikimedia.org", url.path.contains("/thumb/") else { return nil }
            return url
        }
    }
    public let default_language: String
    public let variants: [Variant]
    public let photo: Photo?
    public var article: Variant? {
        variants.first { $0.language == "en" } ?? variants.first { $0.language == default_language }
    }
}
