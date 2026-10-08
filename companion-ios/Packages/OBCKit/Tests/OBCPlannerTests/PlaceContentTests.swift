import Foundation
import Testing
@testable import OBCPlanner

@Suite("Shared planner place content")
struct PlaceContentTests {
    @Test func offlineTextAndCreditsSurviveMissingOnlinePhotoIdentity() async throws {
        let bytes = Data(#"""
        {"default_language":"de","variants":[
          {"language":"de","text_pages":["Eine Burg."],"attribution":{"source_url":"https://de.wikipedia.org/?oldid=1","revision":"1","license_url":"https://creativecommons.org/licenses/by-sa/4.0/"}},
          {"language":"en","text_pages":["A castle."],"attribution":{"source_url":"https://en.wikipedia.org/?oldid=2","revision":"2","license_url":"https://creativecommons.org/licenses/by-sa/4.0/"}}],
         "photo":{"source_url":"https://commons.wikimedia.org/?oldid=3","revision":"3","license_url":"https://creativecommons.org/licenses/by/4.0/","online_url":"https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Castle.jpg/500px-Castle.jpg","credit":["Castle","Artist","CC BY 4.0","Commons"]}}
        """#.utf8)
        let content = try JSONDecoder().decode(PlaceContent.self, from: bytes)
        #expect(content.article?.language == "en")
        #expect(content.article?.text_pages == ["A castle."])
        #expect(content.photo?.credit[1] == "Artist")
        #expect(try await content.photo?.onlineURL() == nil)
    }

    @Test func currentPhotoRequiresBothFileAndDescriptionRevision() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [ContentPhotoHTTP.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let payload = #"""
        {"source_url":"https://commons.wikimedia.org/?oldid=42","revision":"42","license_url":"https://creativecommons.org/licenses/by/4.0/",
         "online_url":"https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Image.jpg/500px-Image.jpg","credit":["Image","Artist","CC BY 4.0","Commons"],
         "file_identity":{"filename":"Image.jpg","page_id":7},"page_revision":42,"file_revision":{"timestamp":"2026-01-01T00:00:00Z","sha1":"abc"}}
        """#
        for (from, to) in [("", ""), ("\"page_revision\":42", "\"page_revision\":43"),
                           ("\"sha1\":\"abc\"", "\"sha1\":\"changed\""), ("\"page_id\":7", "\"page_id\":8")] {
            let changed = from.isEmpty ? payload : payload.replacingOccurrences(of: from, with: to)
            let photo = try JSONDecoder().decode(PlaceContent.Photo.self, from: Data(changed.utf8))
            let url = try await photo.onlineURL(session: session)
            #expect((url != nil) == from.isEmpty)
        }
    }
}

private final class ContentPhotoHTTP: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let body = Data(#"""
        {"query":{"pages":[{"pageid":7,"lastrevid":42,"title":"File:Image.jpg","imageinfo":[
          {"timestamp":"2026-01-01T00:00:00Z","sha1":"abc","thumburl":"https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Image.jpg/500px-Image.jpg"}]}]}}
        """#.utf8)
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: body)
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() { }
}
