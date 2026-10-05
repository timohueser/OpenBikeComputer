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
        let route = try await service.route(points: [a, b], activity: .gravel, preference: .shorter, release: release)
        #expect(release.contains(a))
        #expect(route.points.map(\.coordinate) == [a, b])
        #expect(route.points.map(\.elevationMeters) == [270, nil])
        #expect(route.distance == 900 && route.ascent == 30 && route.seconds == 400)
    }
    @Test(arguments: ["wrong-package", "wrong-profile", "short-elevation", "bad-coordinate", "short-elapsed", "bad-leg"])
    func rejectsUnrelatedOrMalformedRoutes(_ kind: String) async throws {
        let service = client(route: kind), release = try await service.release()
        await #expect(throws: PlannerFailure.invalidData) {
            try await service.route(points: [a, b], activity: .gravel, preference: .shorter, release: release)
        }
    }
    @Test(arguments: [(422, "no_path", PlannerFailure.noRoad), (503, "busy", .busy), (422, "missing_region", .outsideRegion)])
    func reportsFailuresWithoutSubstituteGeometry(_ status: Int, _ code: String, _ failure: PlannerFailure) async throws {
        let service = client(route: code, status: status), release = try await service.release()
        await #expect(throws: failure) { try await service.route(points: [a, b], activity: .gravel, release: release) }
    }
    @Test func eachActivityNamesItsRoutingProfile() {
        #expect(RouteActivity.allCases.map { RoutePreference.balanced.profile(for: $0) } == ["road", "gravel", "mtb", "touring", "hiking"])
        #expect(RoutePreference.lessClimbing.profile(for: .hiking) == "hiking/less-climbing")
        #expect(BikeType.allCases.allSatisfy { RouteActivity($0).bikeType == $0 } && RouteActivity.hiking.bikeType == nil)
    }
    /// The route is the straight line through the request points. A leg position names its point.
    private func lineService(_ sent: Bodies) -> (PlannerService, PlannerRelease) {
        let host = URL(string: "https://planner.test")!
        let release = PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host, places: host,
                                     glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host,
                                     manifest: host.appending(path: "manifest.json"), overlays: host)
        let service = PlannerService(release: release) { request in
            let ok = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
            guard request.url!.lastPathComponent == "route" else {
                return (try JSONSerialization.data(withJSONObject: ["routing_package": packageID, "profiles": ["gravel"]]), ok)
            }
            await sent.append(request.httpBody!)
            let identity = await sent.identity
            let points = try JSONSerialization.jsonObject(with: request.httpBody!) as! [String: Any]
            let line = (points["points"] as! [[Double]]).map { $0.map { Int(($0 * 1e6).rounded()) } }
            let legs = line.indices.dropLast().map { k in
                ["from_index": k, "to_index": k + 1, "start": "\(identity)\(line[k])", "end": "\(identity)\(line[k + 1])",
                 "totals": ["distance_m": 1000, "ascent_m": 10, "seconds": 200]] as [String: Any]
            }
            let route: [String: Any] = ["package": packageID, "profile": "gravel",
                "coordinates_udeg": line.indices.flatMap { k in [0, 1].map { line[k][$0] - (k > 0 ? line[k - 1][$0] : 0) } },
                "elevation_dm": line.map { _ in NSNull() }, "elapsed_s": line.indices.map { $0 > 0 ? 200 : 0 }, "legs": legs,
                "totals": ["distance_m": 1000 * legs.count, "ascent_m": 10 * legs.count, "seconds": 200 * legs.count]]
            return (try JSONSerialization.data(withJSONObject: ["routes": [route]]), ok)
        }
        return (service, release)
    }
    @Test func anEditRequestsOnlyItsLegsPinnedToTheCachedLegs() async throws {
        let sent = Bodies(), (service, release) = lineService(sent)
        let points = (0...4).map { Coordinate(latitude: 48, longitude: 8 + Double($0) / 10) }
        var moved = points
        moved[2] = Coordinate(latitude: 48.05, longitude: 8.2)
        let whole = try await service.route(points: points, activity: .gravel, release: release)
        let edited = try await service.route(points: moved, activity: .gravel, release: release)
        #expect(try await service.route(points: points, activity: .gravel, release: release).points == whole.points)
        let bodies = try await sent.values.map { try JSONSerialization.jsonObject(with: $0) as! [String: Any] }
        #expect(bodies.count == 2 && bodies[0]["start_position"] == nil)
        #expect(bodies[1]["points"] as? [[Double]] == [[8.1, 48], [8.2, 48.05], [8.3, 48]])
        #expect(bodies[1]["start_position"] as? String == "[8100000, 48000000]" && bodies[1]["end_position"] as? String == "[8300000, 48000000]")
        #expect(edited.points.map(\.coordinate) == moved && edited.pointIndices == [0, 1, 2, 3, 4])
        #expect(edited.distance == 4000 && edited.ascent == 40 && edited.seconds == 800 && edited.elapsed.last == 800)
        // A window whose ends do not join the cached legs is followed by one request for the whole route.
        await sent.change(identity: "new")
        moved[2] = Coordinate(latitude: 48.06, longitude: 8.2)
        #expect(try await service.route(points: moved, activity: .gravel, release: release).points.map(\.coordinate) == moved)
        let retried = try await sent.values.suffix(2).map { try JSONSerialization.jsonObject(with: $0) as! [String: Any] }
        #expect(retried[0]["start_position"] as? String == "[8100000, 48000000]")
        #expect(retried[1]["points"] as? [[Double]] == moved.map { [$0.longitude, $0.latitude] } && retried[1]["start_position"] == nil)
    }
    @Test func aTurnaroundIsSentAndStaysInsideTheRequest() async throws {
        let sent = Bodies(), (service, release) = lineService(sent)
        let points = (0...4).map { Coordinate(latitude: 48, longitude: 8 + Double($0) / 10) }
        var moved = points
        moved[3] = Coordinate(latitude: 48.05, longitude: 8.3)
        _ = try await service.route(points: points, turnarounds: [2], activity: .gravel, release: release)
        _ = try await service.route(points: moved, turnarounds: [2], activity: .gravel, release: release)
        // The same points without the turnaround are other legs.
        _ = try await service.route(points: moved, activity: .gravel, release: release)
        let bodies = try await sent.values.map { try JSONSerialization.jsonObject(with: $0) as! [String: Any] }
        #expect(bodies.count == 3 && bodies[0]["turnarounds"] as? [Int] == [2])
        // The edited legs start at the turnaround, so the request starts one point before it.
        #expect(bodies[1]["points"] as? [[Double]] == moved[1...].map { [$0.longitude, $0.latitude] })
        #expect(bodies[1]["turnarounds"] as? [Int] == [1] && bodies[1]["start_position"] as? String == "[8100000, 48000000]")
        #expect(bodies[2]["turnarounds"] == nil)
        await #expect(throws: PlannerFailure.invalidData) {
            try await service.route(points: points, turnarounds: [0], activity: .gravel, release: release)
        }
    }
    @Test func searchUsesCanonicalFiltersAndRouteDistances() async throws {
        let service = client(), release = try await service.release()
        var query = PlannerSearchQuery(text: "camping", view: [7.9,47.9,8.2,48.2])
        query.kinds = ["campsite"]; query.route = [a,b]; query.routeLengthMeters = 900
        query.alongRoute = true; query.fromMeters = 100; query.toMeters = 800; query.radiusMeters = 500
        let places = try await service.search(query, release: release)
        #expect(places.count == 1 && places[0].id == "n123")
        #expect(places[0].kind == "campsite" && places[0].opening_hours == "24/7")
        #expect(places[0].website == "camp.example" && places[0].phone == "+49 123")
        #expect(places[0].description == "Small tents only.")
        #expect(places[0].position?.along == 0.4 && places[0].position?.distance == 0.02)
    }
    @Test func placeDetailsUseAnExactSourceAndSafeContactLinks() async throws {
        let service = client(), release = try await service.release()
        var query = PlannerSearchQuery(text: "Camp")
        query.source = "n123"
        let places = try await service.search(query, release: release)
        #expect(places.first?.source == "n123")
        #expect(PlaceContact.website(places.first?.website)?.absoluteString == "https://camp.example")
        #expect(PlaceContact.phone("+49 (123) 45-67")?.absoluteString == "tel:+491234567")
        #expect(PlaceContact.numbers("+49 123; +33 456") == ["+49 123", "+33 456"])
        for value in ["javascript:alert(1)", "data:text/html,test", "file:///tmp/place", "https://"] {
            #expect(PlaceContact.website(value) == nil)
        }
        #expect(PlaceContact.phone("call reception") == nil)
        query.source = "n456"
        await #expect(throws: PlannerFailure.invalidData) { try await service.search(query, release: release) }
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
            try await service.route(points: [a, Coordinate(latitude: 0, longitude: 0)], activity: .road, release: release)
        }
    }
    /// The line goes out simplified within 10 m with 6 decimals; the answer comes back as plan points.
    @Test(arguments: [(200, nil), (422, PlannerFailure.lineNotReproducible), (422, .lineTooLong)])
    func shapesALineInOneRequest(_ status: Int, _ failure: PlannerFailure?) async throws {
        let host = URL(string: "https://planner.test")!
        let release = PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host, places: host,
                                     glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host, manifest: host, overlays: host)
        let sent = Bodies()
        let service = PlannerService(release: release) { request in
            #expect(request.url?.path == "/v1/shape" && request.timeoutInterval == 40)
            await sent.append(request.httpBody!)
            let body = status == 200 ? #"{"points": [[8, 48], [8.05, 48.05], [8.1, 48.1]], "turnarounds": [1]}"#
                : #"{"code": "\#(failure == .lineTooLong ? "line_too_long" : "line_not_reproducible")", "message": ""}"#
            return (Data(body.utf8), HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }
        let line = [a, Coordinate(latitude: 48.05, longitude: 8.05), Coordinate(latitude: 48.1000000004, longitude: 8.1000000004)]
        if let failure {
            await #expect(throws: failure) { try await service.shape(line: line, profile: "gravel") }
            return
        }
        let shape = try await service.shape(line: line, profile: "gravel")
        #expect(shape.points == [a, Coordinate(latitude: 48.05, longitude: 8.05), b] && shape.turnarounds == [1])
        let query = try #require(try JSONSerialization.jsonObject(with: await sent.values[0]) as? [String: Any])
        #expect(query["profile"] as? String == "gravel" && query["line"] as? [[Double]] == [[8, 48], [8.1, 48.1]])
    }
    /// A 404 from a release object reads the catalogue again, once, and repeats the request with the new active release.
    @Test func aRemovedReleaseReadsTheCatalogueAgainAndRetries() async throws {
        let sent = Bodies()
        let service = PlannerService(catalogURL: URL(string: "https://planner.test/catalog.json")!) { request in
            let url = request.url!.absoluteString
            await sent.append(Data(url.utf8))
            let catalogs = await sent.values.filter { $0 == Data("https://planner.test/catalog.json".utf8) }.count
            let body: [String: Any] = url.hasSuffix("/catalog.json") ? ["format": 1, "active": catalogRelease(catalogs == 1 ? "a" : "b")]
                : ["routing_package": packageID, "profiles": ["gravel"]]
            let status = url.hasPrefix("https://planner.test/a/") ? 404 : 200
            return (try JSONSerialization.data(withJSONObject: body), HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }
        let old = try await service.release()
        let profiles = try await service.profiles(release: old)
        let urls = await sent.values.map { String(decoding: $0, as: UTF8.self) }
        #expect(profiles == ["gravel"])
        #expect(urls == ["https://planner.test/catalog.json", "https://planner.test/a/manifest.json",
                         "https://planner.test/catalog.json", "https://planner.test/b/manifest.json"])
    }
    /// Route catalog cells that answer 404 load from the new active release, after one catalogue read.
    @Test func aRemovedReleaseLoadsItsRouteCellFromTheNewRelease() async throws {
        func release(_ id: String) -> PlannerRelease {
            let host = URL(string: "https://planner.test/\(id)")!
            return PlannerRelease(id: String(repeating: id, count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host, places: host,
                                  glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host,
                                  manifest: host, overlays: host, routes: "https://planner.test/\(id)/{cell}.json")
        }
        let next = release("b"), reads = Bodies()
        let transport: RouteCatalog.Transport = { request in
            let status = request.url!.absoluteString.hasPrefix("https://planner.test/a/") ? 404 : 200
            return (Data(#"{"format": 1, "routes": []}"#.utf8), HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
        }
        let active: @Sendable () async throws -> PlannerRelease = { await reads.append(Data()); return next }
        let made = RouteCatalog(release: release("a"), transport: transport, active: active)
        let catalog = try #require(made)
        async let first = catalog.loadCell("9-267-178")
        async let second = catalog.loadCell("9-268-178")
        let counts = try await [first?.count, second?.count]
        let readCount = await reads.values.count
        #expect(counts == [0, 0])
        #expect(readCount == 1)
    }
    /// A line over the service's 200 km cap fails before any request.
    @Test func aTooLongLineFailsWithoutARequest() async throws {
        let host = URL(string: "https://planner.test")!
        let release = PlannerRelease(id: String(repeating: "a", count: 64), region: "test", bounds: [7, 47, 9, 49], basemap: host, places: host,
                                     glyphs: "", sprites: "", terrain: "", terrain_attribution: "", search: host, routing: host, manifest: host, overlays: host)
        let sent = Bodies()
        let service = PlannerService(release: release) { request in
            await sent.append(request.httpBody ?? Data())
            throw URLError(.badServerResponse)
        }
        await #expect(throws: PlannerFailure.lineTooLong) {
            try await service.shape(line: [a, Coordinate(latitude: 50, longitude: 8)], profile: "gravel")
        }
        #expect(await sent.values.isEmpty)
    }
}

private let a = Coordinate(latitude: 48, longitude: 8)
private let b = Coordinate(latitude: 48.1, longitude: 8.1)
private let packageID = String(repeating: "b", count: 64)
private let goodRoute = "good"

/// A catalogue entry whose objects live under `https://planner.test/ID/`.
private func catalogRelease(_ id: Character) -> [String: Any] {
    let host = "https://planner.test/\(id)"
    return ["id": String(repeating: id, count: 64), "region": "test", "bounds": [7, 47, 9, 49], "basemap": host + "/basemap.json", "places": host + "/places.json",
            "glyphs": host + "/fonts/{fontstack}/{range}.pbf", "sprites": host + "/sprites", "terrain": host + "/{z}/{x}/{y}.webp",
            "terrain_attribution": "Terrain", "search": host + "/search", "routing": host + "/routing",
            "manifest": host + "/manifest.json", "overlays": host + "/overlays.json"]
}

private actor Bodies {
    var values: [Data] = []
    var identity = ""
    func append(_ body: Data) { values.append(body) }
    func change(identity: String) { self.identity = identity }
}

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
                "basemap": host + "/basemap.json", "places": host + "/places.json", "glyphs": host + "/fonts/{fontstack}/{range}.pbf", "sprites": host + "/sprites", "terrain": host + "/{z}/{x}/{y}.webp", "terrain_attribution": "Terrain", "search": host + "/search", "routing": host + "/" + kind, "manifest": host + "/manifest.json" + suffix, "overlays": host + "/overlays.json"]
            data = try! JSONSerialization.data(withJSONObject: ["format": 1, "active": release])
        } else if url.lastPathComponent == "manifest.json" {
            data = try! JSONSerialization.data(withJSONObject: ["routing_package": packageID, "profiles": ["gravel", "gravel/shorter"]])
        } else if url.lastPathComponent == "query" {
            let query = body()
            let criteria = query["request"] as! [String: Any]
            if let source = query["source"] as? String {
                precondition(["n123", "n456"].contains(source) && criteria["type"] as? String == "place")
            } else if query["q"] as? String == "long route" {
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
                "city": "Freiburg", "lon": 8, "lat": 48, "opening_hours": "24/7", "website": "camp.example",
                "phone": "+49 123", "description": "Small tents only.", "position": ["along": 0.4, "distance": 0.02]]]])
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
                    "elevation_dm": heights, "elapsed_s": kind == "short-elapsed" ? [0] : [0,400], "legs": [["from_index": 0, "to_index": kind == "bad-leg" ? 2 : 1,
                        "start": "a", "end": "b", "totals": ["distance_m": 900, "ascent_m": 30, "seconds": 400]]],
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
