import Foundation
import OBCDomain
import Testing
@testable import OBCWeather
@testable import OBCWeatherWire

/// End to end over fixtures: real MET capture, real OBCG vectors, real manifest shape, one OBCW
/// object out. No deployed service and no BLE anywhere in the process.
struct WeatherAssemblerTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    static let baseURL = URL(string: "https://wx.example.invalid/")!

    /// A rider inside the radar vector's window, moving north-east. The fixture grid is only
    /// 40 x 40 cells at 1 km, so the projected two-hour corridor has to fit inside about 40 km —
    /// hence the deliberately gentle speed.
    static func request(capture: WeatherFixtures.METCapture) -> WeatherRequest {
        WeatherRequest(
            requestID: 11,
            position: Coordinate(latitude: 47.15, longitude: 7.25),
            fixTime: now, bearingDegrees: 45, speedMetresPerSecond: 2,
            altitudeMetres: capture.provenance.altitude_m)
    }

    static func assembler(
        manifest: ManifestBuilder, capture: WeatherFixtures.METCapture,
        metStatus: Int? = nil, metOffline: Bool = false
    ) throws -> (WeatherAssembler, StubWeatherHTTPClient, StubWeatherHTTPClient) {
        let serviceHTTP = StubWeatherHTTPClient(objects: try manifest.stubObjects())
        let metHTTP = StubWeatherHTTPClient(objects: [
            "/weatherapi/locationforecast/2.0/complete": StubWeatherHTTPClient.Object(
                bytes: capture.locationforecastJSON(), status: metStatus, offline: metOffline),
        ])
        let assembler = WeatherAssembler(
            hourlyProvider: METLocationforecastAdapter(client: metHTTP),
            precipitationProvider: OBCWeatherServiceClient(
                baseURL: baseURL, client: serviceHTTP))
        return (assembler, serviceHTTP, metHTTP)
    }

    static func radarManifest(stalenessDeadline: TimeInterval = 900) throws -> ManifestBuilder {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "dwd-rv", tier: 1, vectors: ["grid-multipage.obcg"],
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(stalenessDeadline)))
        return builder
    }

    @Test
    func aCoveredCorridorProducesHourlyPlusRainWithBothAttributions() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, met) = try Self.assembler(
            manifest: try Self.radarManifest(), capture: capture)
        let built = try await assembler.assemble(
            request: Self.request(capture: capture), generation: 3, now: Self.now)

        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.count == 1)
        #expect(built.bundle.requestID == 11)
        #expect(built.state.precipitation?.productID == "dwd-rv")
        #expect(built.state.noRainMapReason == nil)
        #expect(built.state.attributions.contains(.met))
        #expect(built.bytes.count <= OBCWeatherCodec.producerPolicyMaximumLength)
        // The bytes are a valid OBCW object by the wire codec's own rules.
        #expect(try OBCWeatherCodec.decode(built.bytes) == built.bundle)

        // No rider coordinate ever reaches OBC infrastructure; only MET receives one.
        for request in service.requests {
            #expect(!request.url.absoluteString.contains("lat"))
            #expect(request.url.query == nil)
        }
        #expect(met.requests.first?.url.query?.contains("lat=47.1500") == true)
        #expect(built.state.diagnostics.serviceRequests > 0)
        #expect(built.state.diagnostics.serviceBytes > 0)
    }

    /// The independence rule, end to end: the rain half fails completely and the hourly forecast
    /// still ships, labelled with why there is no map.
    @Test
    func aServiceOutageStillShipsTheHourlyForecast() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, _) = try Self.assembler(
            manifest: try Self.radarManifest(), capture: capture)
        service.mutate(OBCWeatherServiceClient.manifestKey) { $0.offline = true }
        let built = try await assembler.assemble(
            request: Self.request(capture: capture), generation: 1, now: Self.now)
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .serviceUnavailable)
        #expect(built.state.precipitation == nil)
    }

    @Test
    func anExpiredProductYieldsHourlyOnlyRatherThanStaleRain() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, _, _) = try Self.assembler(
            manifest: try Self.radarManifest(stalenessDeadline: -60), capture: capture)
        let built = try await assembler.assemble(
            request: Self.request(capture: capture), generation: 1, now: Self.now)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason
            == .allCoveringProductsExpired(latestDeadline: Self.now.addingTimeInterval(-60)))
        #expect(built.state.diagnostics.expiredCoveringProducts == ["dwd-rv"])
    }

    @Test
    func aGapRegionYieldsTheExplicitNoRainMapState() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-manila-24h.json")
        let (assembler, _, _) = try Self.assembler(
            manifest: try Self.radarManifest(), capture: capture)
        let manila = WeatherRequest(
            requestID: 2,
            position: Coordinate(
                latitude: capture.provenance.latitude, longitude: capture.provenance.longitude),
            fixTime: Self.now, altitudeMetres: capture.provenance.altitude_m)
        let built = try await assembler.assemble(request: manila, generation: 1, now: Self.now)
        #expect(built.bundle.hourly.count == 24)
        #expect(built.bundle.rainFrames.isEmpty)
        #expect(built.state.noRainMapReason == .corridorNotCovered)
        // A worldwide coordinate still gets 24 valid hours and MET's attribution.
        #expect(built.state.attributions == [.met])
    }

    /// Without hourly there is no bundle at all — the device keeps whatever it already holds.
    @Test
    func aFailedHourlyFetchFailsTheJob() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, _, _) = try Self.assembler(
            manifest: try Self.radarManifest(), capture: capture, metOffline: true)
        await #expect(throws: (any Error).self) {
            try await assembler.assemble(
                request: Self.request(capture: capture), generation: 1, now: Self.now)
        }
    }

    @Test
    func theWholeJobIsReproducibleFromTheSameFixtures() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (first, _, _) = try Self.assembler(manifest: try Self.radarManifest(), capture: capture)
        let (second, _, _) = try Self.assembler(manifest: try Self.radarManifest(), capture: capture)
        let request = Self.request(capture: capture)
        let a = try await first.assemble(request: request, generation: 5, now: Self.now)
        let b = try await second.assemble(request: request, generation: 5, now: Self.now)
        #expect(a.bytes == b.bytes)
    }

    @Test
    func aRequestWithoutAFixNeverStartsAJob() async throws {
        let capture = try WeatherFixtures.metCapture("met-locationforecast-oslo-24h.json")
        let (assembler, service, met) = try Self.assembler(
            manifest: try Self.radarManifest(), capture: capture)
        await #expect(throws: WeatherProviderError.noPosition) {
            try await assembler.assemble(
                request: WeatherRequest(requestID: 9), generation: 1, now: Self.now)
        }
        #expect(service.requests.isEmpty)
        #expect(met.requests.isEmpty)
    }
}
