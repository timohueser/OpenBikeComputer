import Foundation
import OBCDomain
import Testing
@testable import OBCPlanner

private let cafe = Coordinate(latitude: 47.99468, longitude: 7.84970)

@Suite("Places shared from map apps")
struct SharedPlaceTests {
    @Test(arguments: [
        ("https://maps.apple.com/?address=Kaiser-Joseph-Stra%C3%9Fe%20200,%2079098%20Freiburg,%20Germany&auid=123&ll=47.99468,7.84970&lsp=9902&q=Caf%C3%A9%20Atlantik",
         SharedPlace.coordinate(cafe, name: "Café Atlantik")),
        ("https://maps.apple.com/place?address=Kaiser-Joseph-Stra%C3%9Fe%20200,%2079098%20Freiburg,%20Germany&coordinate=47.99468,7.84970&name=Caf%C3%A9%20Atlantik&place-id=I123",
         .coordinate(cafe, name: "Café Atlantik")),
        ("https://maps.apple.com/?ll=47.9,7.8", .coordinate(Coordinate(latitude: 47.9, longitude: 7.8), name: nil)),
        ("https://maps.apple.com/?q=Titisee", .query("Titisee")),
        ("https://maps.apple/p/AbCdEf", .shortLink(URL(string: "https://maps.apple/p/AbCdEf")!, name: nil)),
        ("Café Atlantik\nKaiser-Joseph-Straße 200, 79098 Freiburg im Breisgau\nhttps://maps.app.goo.gl/AbC123xyz",
         .shortLink(URL(string: "https://maps.app.goo.gl/AbC123xyz")!, name: "Café Atlantik")),
        ("https://goo.gl/maps/XyZ789", .shortLink(URL(string: "https://goo.gl/maps/XyZ789")!, name: nil)),
        ("https://www.google.com/maps/place/Caf%C3%A9+Atlantik/@47.9946,7.8490,17z/data=!3m1!4b1!4m6!3m5!1s0x0:0x0!8m2!3d47.99468!4d7.84970!16s",
         .coordinate(cafe, name: "Café Atlantik")),
        ("https://maps.google.com/?q=47.99468,7.84970", .coordinate(cafe, name: nil)),
        ("https://www.google.com/maps/search/?api=1&query=47.9,7.8", .coordinate(Coordinate(latitude: 47.9, longitude: 7.8), name: nil)),
        ("https://www.google.com/maps?q=Titisee", .query("Titisee")),
        ("https://www.google.com/maps/search/47.99,+7.85?entry=tts", .coordinate(Coordinate(latitude: 47.99, longitude: 7.85), name: nil)),
        ("https://www.google.com/maps/search/Titisee", .query("Titisee")),
        ("https://consent.google.com/m?continue=https://www.google.com/maps/place/Caf%25C3%25A9%2BAtlantik/data%3D!4m2!3d47.99468!4d7.84970%3Fentry%3Dttu&gl=DE&hl=de",
         .coordinate(cafe, name: "Café Atlantik")),
        ("Titisee\n  Hinterzarten  \n", .query("Titisee, Hinterzarten")),
        ("https://maps.apple.com/?ll=95.0,7.8", nil),
        ("https://www.example.com/news/article-123", nil),
        (" \n ", nil),
    ] as [(String, SharedPlace?)])
    func decodesShareText(text: String, place: SharedPlace?) {
        #expect(SharedPlace(text: text) == place)
    }

    @Test func shortLinkStopsAtTheConsentRedirectAndKeepsTheSharedName() async throws {
        let place = try #require(SharedPlace(text: "Café Atlantik\nhttps://maps.app.goo.gl/AbC123xyz"))
        #expect(try await SharedPlaceResolver(session: stubSession()).resolve(place) == .coordinate(cafe, name: "Café Atlantik"))
    }

    @Test(arguments: ["https://maps.app.goo.gl/Loop", "https://maps.app.goo.gl/Gone"])
    func shortLinkWithoutPlaceThrows(link: String) async {
        let url = URL(string: link)!
        await #expect(throws: UnresolvedSharedLink(url: url)) {
            try await SharedPlaceResolver(session: stubSession()).resolve(.shortLink(url, name: nil))
        }
    }
}

private func stubSession() -> URLSession {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [ShortLinkHTTP.self]
    return URLSession(configuration: configuration)
}

/// Answers with the redirect itself, so the test sees each hop the resolver requests.
private final class ShortLinkHTTP: URLProtocol, @unchecked Sendable {
    private static let redirects = [
        "https://maps.app.goo.gl/AbC123xyz": "https://consent.google.com/m?continue=https://www.google.com/maps/%4047.99468,7.84970,17z&gl=DE",
        "https://maps.app.goo.gl/Loop": "/Loop",
    ]

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let url = request.url!
        let location = Self.redirects[url.absoluteString]
        let response = HTTPURLResponse(url: url, statusCode: location == nil ? 404 : 302, httpVersion: nil,
                                       headerFields: location.map { ["Location": $0] })!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() { }
}
