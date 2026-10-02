import Foundation
import OBCDomain
import Testing
@testable import OBCPlanner

@Suite("Published planner service")
struct PlannerServiceTests {
    private func client(route: String = goodRoute, status: Int = 200) -> PlannerService {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubHTTP.self]
        return PlannerService(catalogURL: URL(string: "https://planner.test/\(status)/catalog.json?route=\(route.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!)")!, session: URLSession(configuration: configuration))
    }
    @Test func decodesCoordinatesElevationAndServerTotals() async throws {
        let service = client(), release = try await service.release()
        let route = try await service.route(points: [a, b], bike: .gravel, preference: .shorter, release: release)
        #expect(release.contains(a))
        #expect(route.points.map(\.coordinate) == [a, b])
        #expect(route.points.map(\.elevationMeters) == [270, nil])
        #expect(route.distance == 900 && route.ascent == 30 && route.seconds == 400)
    }
    @Test(arguments: ["wrong-package", "wrong-profile", "short-elevation", "bad-coordinate", "short-elapsed", "bad-leg"])
    func rejectsUnrelatedOrMalformedRoutes(_ kind: String) async throws {
        let service = client(route: kind), release = try await service.release()
        await #expect(throws: PlannerFailure.invalidData) {
            try await service.route(points: [a, b], bike: .gravel, preference: .shorter, release: release)
        }
    }
    @Test(arguments: [(422, "no_path", PlannerFailure.noRoad), (503, "busy", .busy), (422, "missing_region", .outsideRegion)])
    func reportsFailuresWithoutSubstituteGeometry(_ status: Int, _ code: String, _ failure: PlannerFailure) async throws {
        let service = client(route: code, status: status), release = try await service.release()
        await #expect(throws: failure) { try await service.route(points: [a, b], bike: .gravel, release: release) }
    }
    @Test func searchUsesCanonicalFiltersAndRouteDistances() async throws {
        let service = client(), release = try await service.release()
        var query = PlannerSearchQuery(text: "camping", view: [7.9,47.9,8.2,48.2])
        query.kinds = ["campsite"]; query.route = [a,b]; query.routeLengthMeters = 900
        query.alongRoute = true; query.fromMeters = 100; query.toMeters = 800; query.radiusMeters = 500
        let places = try await service.search(query, release: release)
        #expect(places.count == 1 && places[0].id == "n123")
        #expect(places[0].kind == "campsite" && places[0].opening_hours == "24/7")
        #expect(places[0].position?.along == 0.4 && places[0].position?.distance == 0.02)
    }
    @Test func overlaysKeepOnlyNativeStyleDataAndCheckReleaseIdentity() async throws {
        let service = client(), release = try await service.release()
        let data = try await service.overlays(bounds: [7.9,47.9,8.2,48.2], zoom: 13.7, network: "hiking", release: release)
        let collection = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        let feature = try #require((collection["features"] as? [[String: Any]])?.first)
        let properties = try #require(feature["properties"] as? [String: Any])
        #expect(properties["kind"] as? String == "hiking" && properties["rank"] as? Int == 2)
        #expect(properties["tags"] == nil && collection["routes"] == nil)
    }
    @Test func longRoutesFitTheSearchContractAndKeepTheirEnds() async throws {
        let service = client(), release = try await service.release()
        var query = PlannerSearchQuery(text: "long route")
        query.route = (0...20_000).map { index in
            let fraction = Double(index) / 20_000
            return Coordinate(latitude: 48 + 0.1 * fraction, longitude: 8 + 0.1 * fraction)
        }
        let places = try await service.search(query, release: release)
        #expect(places.count == 1)
    }
    @Test func rejectsPointsBeyondTheReleaseBeforeRouting() async throws {
        let service = client(), release = try await service.release()
        await #expect(throws: PlannerFailure.outsideRegion) {
            try await service.route(points: [a, Coordinate(latitude: 0, longitude: 0)], bike: .road, release: release)
        }
    }
}

private let a = Coordinate(latitude: 48, longitude: 8)
private let b = Coordinate(latitude: 48.1, longitude: 8.1)
private let packageID = String(repeating: "b", count: 64)
private let goodRoute = "good"

private final class StubHTTP: URLProtocol, @unchecked Sendable {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let url = request.url!
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        var code = 200
        let data: Data
        if url.lastPathComponent == "catalog.json" {
            let host = "https://planner.test/" + url.path.split(separator: "/")[0]
            let kind = components.queryItems!.first!.value!
            let suffix = "?route=" + kind.addingPercentEncoding(withAllowedCharacters: .alphanumerics)!
            let release: [String: Any] = ["id": String(repeating: "a", count: 64), "region": "test", "bounds": [7, 47, 9, 49],
                "basemap": host + "/basemap.json", "glyphs": host + "/fonts/{fontstack}/{range}.pbf", "sprites": host + "/sprites", "terrain": host + "/{z}/{x}/{y}.webp", "terrain_attribution": "Terrain", "search": host + "/search", "routing": host + "/" + kind, "manifest": host + "/manifest.json" + suffix]
            data = try! JSONSerialization.data(withJSONObject: ["format": 1, "active": release])
        } else if url.lastPathComponent == "manifest.json" {
            data = try! JSONSerialization.data(withJSONObject: ["routing_package": packageID, "profiles": ["gravel", "gravel/shorter"]])
        } else if url.lastPathComponent == "query" {
            let query = body()
            let criteria = query["request"] as! [String: Any]
            if query["q"] as? String == "long route" {
                let coordinates = (query["plan"] as! [String: Any])["coordinates"] as! [[Double]]
                precondition(coordinates.count <= 20_000 && coordinates.first == [8,48] && coordinates.last == [8.1,48.1])
                precondition(criteria["type"] as? String == "place" && criteria["where"] == nil)
            } else {
            let scope = criteria["where"] as! [String: Any], along = scope["along"] as! [String: Any]
            let plan = query["plan"] as! [String: Any], day = (plan["days"] as! [[String: Any]])[0]
            precondition(request.httpMethod == "POST" && criteria["what"] as? [String] == ["campsite"])
            precondition(criteria["type"] as? String == "places" && scope["scope"] as? String == "route")
            precondition(query["view"] as? [Double] == [7.9,47.9,8.2,48.2] && day["to"] as? Double == 0.9)
            precondition((along["from"] as? [String: Any])?["value"] as? Double == 0.1)
            precondition((criteria["radius"] as? [String: Any])?["value"] as? Double == 0.5 && (criteria["radius"] as? [String: Any])?["unit"] as? String == "km")
            }
            data = try! JSONSerialization.data(withJSONObject: ["results": [["source": "n123", "name": "Camp", "kind": "campsite",
                "city": "Freiburg", "lon": 8, "lat": 48, "opening_hours": "24/7", "position": ["along": 0.4, "distance": 0.02]]]])
        } else if url.lastPathComponent == "overlays" {
            let params = Dictionary(uniqueKeysWithValues: components.queryItems!.map { ($0.name, $0.value!) })
            precondition(request.httpMethod == "GET" && params["layers"] == "hiking" && params["zoom"] == "13.0" && params["mode"] == "walking")
            data = try! JSONSerialization.data(withJSONObject: ["type": "FeatureCollection", "package": packageID,
                "routes": ["123": ["name": "Trail"]], "features": [["type": "Feature",
                "geometry": ["type": "LineString", "coordinates": [[8,48],[8.1,48.1]]],
                "properties": ["kind": "hiking", "rank": 2, "ref": "Trail", "tags": ["name": "Trail"]]]]])
        } else {
            code = Int(url.path.split(separator: "/")[0])!
            let kind = String(url.path.split(separator: "/")[1])
            if code != 200 { data = try! JSONSerialization.data(withJSONObject: ["code": kind, "message": "Failure"]) }
            else {
                let query = body()
                // This fake server requires longitude first and the requested profile.
                guard (query["profile"] as? String) == "gravel/shorter", (query["points"] as? [[Double]]) == [[8, 48], [8.1, 48.1]], query["alternatives"] as? Bool == false else { fatalError("Wrong route request") }
                let heights: [Any] = kind == "short-elevation" ? [2700] : [2700, NSNull()]
                let route: [String: Any] = ["package": kind == "wrong-package" ? "other" : packageID,
                    "profile": kind == "wrong-profile" ? "road" : "gravel/shorter",
                    "coordinates_udeg": kind == "bad-coordinate" ? [800_000_000, 48_000_000, -791_900_000, 100_000] : [8_000_000, 48_000_000, 100_000, 100_000],
                    "elevation_dm": heights, "elapsed_s": kind == "short-elapsed" ? [0] : [0,400], "legs": [["from_index": 0, "to_index": kind == "bad-leg" ? 2 : 1]],
                    "totals": ["distance_m": 900, "ascent_m": 30, "seconds": 400]]
                data = try! JSONSerialization.data(withJSONObject: ["routes": [route]])
            }
        }
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: url, statusCode: code, httpVersion: nil, headerFields: nil)!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
    private func body() -> [String: Any] {
        try! JSONSerialization.jsonObject(with: request.httpBody ?? streamData(request.httpBodyStream!)) as! [String: Any]
    }
    override func stopLoading() {}
    private func streamData(_ stream: InputStream) -> Data {
        stream.open(); defer { stream.close() }
        var data = Data(), buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { return data }
            data.append(contentsOf: buffer.prefix(count))
        }
    }
}
