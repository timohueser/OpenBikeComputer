import Foundation
import Testing
@testable import OBCWeather

/// WX13's provenance seam: the manifest, read as health + credits.
///
/// The suite's real subject is the epic's law — *adding a source is a baker deploy*. Every
/// assertion below is about the client rendering products it has never heard of, crediting whoever
/// the manifest says, and refusing to invent freshness.
struct WeatherServiceStatusTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    static let baseURL = URL(string: "https://wx.example.invalid/")!

    static func builder(
        products: [ManifestBuilder.ProductSpec]
    ) throws -> ManifestBuilder {
        var builder = ManifestBuilder()
        builder.generatedAt = now.addingTimeInterval(-120)
        for product in products { try builder.add(product) }
        return builder
    }

    static func spec(
        id: String, tier: UInt8, staleness: TimeInterval, credit: String,
        url: String = "https://creativecommons.org/licenses/by/4.0/"
    ) -> ManifestBuilder.ProductSpec {
        ManifestBuilder.ProductSpec(
            id: id, tier: tier, vectors: ["grid-multipage.obcg"],
            referenceTime: now.addingTimeInterval(-300), generatedAt: now.addingTimeInterval(-120),
            stalenessDeadline: now.addingTimeInterval(staleness),
            attributionText: credit, attributionURL: url)
    }

    static func client(_ builder: ManifestBuilder) throws -> OBCWeatherServiceClient {
        OBCWeatherServiceClient(
            baseURL: baseURL, client: StubWeatherHTTPClient(objects: try builder.stubObjects()))
    }

    /// A product id, tier and credit this build has never seen render exactly as the manifest
    /// states them. Nothing in the app matches on `id`, so "a new source needs no app release" is
    /// not a promise — it is the only thing the code can do.
    @Test func aProductThisBuildHasNeverHeardOfRendersFromTheManifestAlone() async throws {
        let builder = try Self.builder(products: [
            Self.spec(
                id: "geosphere-inca-at", tier: 1, staleness: 900,
                credit: "Source: GeoSphere Austria", url: "https://example.invalid/inca"),
        ])
        let status = try await Self.client(builder).serviceStatus(now: Self.now)
        #expect(status.products.count == 1)
        #expect(status.products[0].id == "geosphere-inca-at")
        #expect(status.products[0].tier == .radar)
        #expect(status.products[0].attribution.text == "Source: GeoSphere Austria")
        #expect(status.attributions.map(\.text) == ["Source: GeoSphere Austria"])
        #expect(status.products[0].isFresh(at: Self.now))
    }

    /// Two products from one upstream credit it once — a rider reads a list of sources, not a list
    /// of products.
    @Test func oneUpstreamIsCreditedOnceAcrossItsProducts() async throws {
        let builder = try Self.builder(products: [
            Self.spec(id: "dwd-rv", tier: 1, staleness: 900, credit: "Source: DWD"),
            Self.spec(id: "dwd-icon-eu", tier: 2, staleness: 3_600, credit: "Source: DWD"),
        ])
        let status = try await Self.client(builder).serviceStatus(now: Self.now)
        #expect(status.products.count == 2)
        #expect(status.attributions.map(\.text) == ["Source: DWD"])
    }

    /// Staleness is per product and past-tense: the baker can be behind on one region while every
    /// other is current, and this must never read as one global "fine" or "broken".
    @Test func stalenessIsStatedPerProductAndNeverAveraged() async throws {
        let builder = try Self.builder(products: [
            Self.spec(id: "dwd-rv", tier: 1, staleness: -60, credit: "Source: DWD"),
            Self.spec(id: "gfs-floor", tier: 3, staleness: 7_200, credit: "Source: NOAA"),
        ])
        let status = try await Self.client(builder).serviceStatus(now: Self.now)
        let stale = status.staleProducts(at: Self.now)
        #expect(stale.map(\.id) == ["dwd-rv"])
        #expect(status.products.first { $0.id == "gfs-floor" }?.isFresh(at: Self.now) == true)
        // Both are still listed and both still credited: a stale product is not a hidden one.
        #expect(status.attributions.count == 2)
    }

    /// An unreachable manifest throws rather than reporting an empty, healthy-looking service —
    /// the screen's "Unavailable" state exists precisely so this cannot be drawn as "no sources".
    @Test func anUnreachableManifestIsAnOutageNotAnEmptyServiceReport() async throws {
        var objects = try Self.builder(products: [
            Self.spec(id: "dwd-rv", tier: 1, staleness: 900, credit: "Source: DWD"),
        ]).stubObjects()
        objects[OBCWeatherServiceClient.manifestKey]?.offline = true
        let client = OBCWeatherServiceClient(
            baseURL: Self.baseURL, client: StubWeatherHTTPClient(objects: objects))
        await #expect(throws: (any Error).self) {
            _ = try await client.serviceStatus(now: Self.now)
        }
    }

    /// The frame table's genuine timestamps come through untouched: freshness is the upstream's
    /// word, never a recomputation.
    @Test func frameCountsAndLatestTimestampComeFromTheManifestFrames() async throws {
        let builder = try Self.builder(products: [
            Self.spec(id: "dwd-rv", tier: 1, staleness: 900, credit: "Source: DWD"),
        ])
        let status = try await Self.client(builder).serviceStatus(now: Self.now)
        let product = try #require(status.products.first)
        #expect(product.frameCount == 1)
        #expect(product.latestFrameValidAt != nil)
        #expect(product.referenceTime == Self.now.addingTimeInterval(-300))
    }
}
