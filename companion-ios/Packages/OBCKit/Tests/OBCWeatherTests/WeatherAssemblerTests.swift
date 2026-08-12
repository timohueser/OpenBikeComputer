import Foundation
import OBCDomain
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// End to end over fixtures: real MET capture, real OBCG shard bytes, real manifest-v2 shape, one
/// OBCW object out. No deployed service and no BLE anywhere in the process.
struct WeatherAssemblerTests {
    static let now = ManifestV2Builder.referenceDate
    static let baseURL = URL(string: "https://wx.example.invalid/")!

    /// A rider in the middle of the fixture lattice. The 90 km disc is wider than the 0.64° lattice,
    /// so the corridor is answered for the part that exists and flagged partial — which is the
    /// ordinary case for a regional bake, and the honest one.
    static func request(capture: WeatherFixtures.METCapture) -> WeatherRequest {
        WeatherRequest(
            requestID: 11, position: Coordinate(latitude: 47.3, longitude: 7.3),
            fixTime: now, altitudeMetres: capture.provenance.altitude_m)
    }

    static func assembler(
        manifest: ManifestV2Builder, capture: WeatherFixtures.METCapture,
        metStatus: Int? = nil, metOffline: Bool = false
    ) throws -> (WeatherAssembler, StubWeatherHTTPClient, StubWeatherHTTPClient) {
        let lastModified = try #require(RFC3339.parse(capture.provenance.last_modified))
        let expires = try #require(RFC3339.parse(capture.provenance.expires))
        let serviceHTTP = StubWeatherHTTPClient(objects: try manifest.stubObjects())
        let metHTTP = StubWeatherHTTPClient(objects: [
            "/weatherapi/locationforecast/2.0/complete": StubWeatherHTTPClient.Object(
                bytes: capture.locationforecastJSON(),
                headers: [
                    "Last-Modified": HTTPDate.string(from: lastModified),
                    "Expires": HTTPDate.string(from: expires),
                ],
                status: metStatus, offline: metOffline),
        ])
        let assembler = WeatherAssembler(
            hourlyProvider: METLocationforecastAdapter(client: metHTTP),
            precipitationProvider: OBCWeatherServiceClient(
                baseURL: baseURL, client: serviceHTTP))
        return (assembler, serviceHTTP, metHTTP)
    }

    @Test
    func aCoveredCorridorProducesHourlyPlusRainWithBothAttributions() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, met) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture)
        let built = try await assembler.assemble(
            request: Self.request(capture: capture), generation: 3, now: Self.now)

        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.count == 2)
        #expect(built.bundle.requestID == 11)
        #expect(built.state.precipitation?.generation == "20260810T1430Z")
        #expect(built.state.noRainMapReason == nil)
        #expect(built.state.attributions.contains(.met))
        #expect(built.state.attributions.count == 2, "MET plus the dataset's one credit")
        #expect(built.bytes.count <= OBCWeatherCodec.producerPolicyMaximumLength)
        // The bytes are a valid OBCW object by the wire codec's own rules.
        #expect(try OBCWeatherCodec.decode(built.bytes) == built.bundle)

        // No rider coordinate ever reaches OBC infrastructure; only MET receives one.
        for request in service.requests {
            #expect(!request.url.absoluteString.contains("lat"))
            #expect(request.url.query == nil)
        }
        #expect(met.requests.first?.url.query?.contains("lat=47.3000") == true)
        #expect(built.state.diagnostics.serviceRequests > 0)
        #expect(built.state.diagnostics.serviceBytes > 0)
    }

    /// The independence rule, end to end: the rain half fails completely and the hourly forecast
    /// still ships, labelled with why there is no map.
    @Test
    func aServiceOutageStillShipsTheHourlyForecast() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, _) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture)
        service.mutate(OBCWeatherServiceClient.manifestKey) { $0.offline = true }
        let built = try await assembler.assemble(
            request: Self.request(capture: capture), generation: 1, now: Self.now)
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .serviceUnavailable)
        #expect(built.state.precipitation == nil)
    }

    @Test
    func anExpiredGenerationYieldsHourlyOnlyRatherThanStaleRain() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let builder = ManifestV2Builder()
        let (assembler, _, _) = try Self.assembler(manifest: builder, capture: capture)
        let built = try await assembler.assemble(
            request: Self.request(capture: capture), generation: 1,
            now: builder.staleAfter.addingTimeInterval(60))
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .expired(staleAfter: builder.staleAfter))
    }

    /// A rider the dataset's lattice does not reach. Not "no rain" — the honest sentence is that the
    /// rain map does not go there.
    @Test
    func aRegionOffTheLatticeYieldsTheExplicitNoRainMapState() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-manila-24h.json")
        let (assembler, _, _) = try Self.assembler(manifest: ManifestV2Builder(), capture: capture)
        let manila = WeatherRequest(
            requestID: 2,
            position: Coordinate(
                latitude: capture.provenance.latitude, longitude: capture.provenance.longitude),
            fixTime: Self.now, altitudeMetres: capture.provenance.altitude_m)
        let built = try await assembler.assemble(request: manila, generation: 1, now: Self.now)
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .outOfDomain)
        // A worldwide coordinate still gets 24 valid hours and MET's attribution.
        #expect(built.state.attributions == [.met])
    }

    /// Without hourly there is no bundle at all — the device keeps whatever it already holds.
    @Test
    func aFailedHourlyFetchFailsTheJob() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, _, _) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture, metOffline: true)
        await #expect(throws: (any Error).self) {
            try await assembler.assemble(
                request: Self.request(capture: capture), generation: 1, now: Self.now)
        }
    }

    @Test
    func theWholeJobIsReproducibleFromTheSameFixtures() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (first, _, _) = try Self.assembler(manifest: ManifestV2Builder(), capture: capture)
        let (second, _, _) = try Self.assembler(manifest: ManifestV2Builder(), capture: capture)
        let request = Self.request(capture: capture)
        let a = try await first.assemble(request: request, generation: 5, now: Self.now)
        let b = try await second.assemble(request: request, generation: 5, now: Self.now)
        #expect(a.bytes == b.bytes)
    }

    @Test
    func aRequestWithoutAFixNeverStartsAJob() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, met) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture)
        await #expect(throws: WeatherProviderError.noPosition) {
            try await assembler.assemble(
                request: WeatherRequest(requestID: 9), generation: 1, now: Self.now)
        }
        #expect(service.requests.isEmpty)
        #expect(met.requests.isEmpty)
    }

    @Test
    func unchangedProviderRevisionsAvoidAllShardReads() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, met) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture)

        let outcome = try await assembler.assembleIfChanged(
            request: Self.request(capture: capture), generation: 4,
            heldBundleGeneratedAt: Self.now, allowHeldBundleReuse: true, now: Self.now)

        guard case .unchanged = outcome else {
            Issue.record("expected the conditional probes to prove the held bundle current")
            return
        }
        #expect(service.requests.count == 1, "only the small manifest is read")
        #expect(service.requests.first?.byteRange == nil)
        #expect(met.requests.count == 1, "the hourly endpoint is conditionally cached by its adapter")
    }

    @Test
    func anOlderHeldBundleFallsThroughToTheFullCorridorBuild() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, _) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture)

        let outcome = try await assembler.assembleIfChanged(
            request: Self.request(capture: capture), generation: 4,
            heldBundleGeneratedAt: Date(timeIntervalSince1970: 1),
            allowHeldBundleReuse: true, now: Self.now)

        guard case let .bundle(built) = outcome else {
            Issue.record("expected a newer provider revision to rebuild")
            return
        }
        #expect(!built.bundle.rainFrames.isEmpty)
        #expect(service.requests.contains { $0.byteRange != nil }, "new rain data reads shards")
    }

    @Test
    func aFailedRevisionProbeStillShipsHourlyOnly() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, _) = try Self.assembler(
            manifest: ManifestV2Builder(), capture: capture)
        service.mutate(OBCWeatherServiceClient.manifestKey) { $0.offline = true }

        let outcome = try await assembler.assembleIfChanged(
            request: Self.request(capture: capture), generation: 4,
            heldBundleGeneratedAt: Self.now, allowHeldBundleReuse: true, now: Self.now)

        guard case let .bundle(built) = outcome else {
            Issue.record("a rain-service outage must not suppress valid hourly weather")
            return
        }
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .serviceUnavailable)
    }
}
