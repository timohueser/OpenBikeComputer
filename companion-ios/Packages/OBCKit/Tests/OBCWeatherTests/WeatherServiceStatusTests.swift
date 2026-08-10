import Foundation
import Testing
@testable import OBCWeather

/// WX13's provenance seam: the manifest, read as health + credits.
///
/// The suite's real subject is the epic's law — *adding a source is a baker deploy*. Every assertion
/// below is about the client crediting sources it has never heard of, reading the deadlines the
/// document states, and refusing to invent freshness. What it no longer asserts is a per-product
/// story: there is one dataset, so "which product answered" is not a question the screen can ask
/// (#1244).
struct WeatherServiceStatusTests {
    static let now = ManifestV2Builder.referenceDate
    static let baseURL = URL(string: "https://wx.example.invalid/")!

    static func client(_ builder: ManifestV2Builder) throws -> OBCWeatherServiceClient {
        OBCWeatherServiceClient(
            baseURL: baseURL, client: StubWeatherHTTPClient(objects: try builder.stubObjects()))
    }

    /// A credit this build has never seen renders exactly as the manifest states it. Nothing in the
    /// app matches on a source id, so "a new source needs no app release" is not a promise — it is
    /// the only thing the code can do.
    @Test func aSourceThisBuildHasNeverHeardOfIsCreditedFromTheManifestAlone() async throws {
        var builder = ManifestV2Builder()
        builder.generatedAt = Self.now.addingTimeInterval(-120)
        let document = try JSONSerialization.jsonObject(with: builder.json()) as! [String: Any]
        var mutated = document
        mutated["attribution"] = [
            ["source_id": "geosphere-inca-at", "text": "Source: GeoSphere Austria",
             "url": "https://example.invalid/inca"],
            ["source_id": "dwd-rv", "text": "Source: Deutscher Wetterdienst (DWD)",
             "url": "https://creativecommons.org/licenses/by/4.0/"],
        ]
        var objects = try builder.stubObjects()
        objects[OBCWeatherServiceClient.manifestKey] = StubWeatherHTTPClient.Object(
            bytes: try JSONSerialization.data(withJSONObject: mutated))
        let client = OBCWeatherServiceClient(
            baseURL: Self.baseURL, client: StubWeatherHTTPClient(objects: objects))

        let status = try await client.serviceStatus(now: Self.now)
        #expect(status.attributions.map(\.sourceID) == ["geosphere-inca-at", "dwd-rv"])
        #expect(status.attributions.first?.text == "Source: GeoSphere Austria")
        #expect(status.generation == "20260810T1430Z")
        #expect(status.isFresh(at: Self.now))
    }

    /// **Every source of the mosaic is credited, on every frame.** There is no per-cell provenance,
    /// so narrowing the list to "the one that answered" would be a claim the data cannot support.
    @Test func everySourceOfTheMosaicIsCredited() async throws {
        var builder = ManifestV2Builder()
        let document = try JSONSerialization.jsonObject(with: builder.json()) as! [String: Any]
        var mutated = document
        mutated["attribution"] = [
            ["source_id": "dwd-rv", "text": "Source: DWD", "url": "https://example.invalid/dwd"],
            ["source_id": "us", "text": "Source: NOAA/NWS MRMS",
             "url": "https://example.invalid/us"],
            ["source_id": "gfs", "text": "Source: NOAA GFS", "url": "https://example.invalid/gfs"],
        ]
        builder.generatedAt = Self.now
        var objects = try builder.stubObjects()
        objects[OBCWeatherServiceClient.manifestKey] = StubWeatherHTTPClient.Object(
            bytes: try JSONSerialization.data(withJSONObject: mutated))
        let client = OBCWeatherServiceClient(
            baseURL: Self.baseURL, client: StubWeatherHTTPClient(objects: objects))
        let status = try await client.serviceStatus(now: Self.now)
        #expect(status.attributions.count == 3)
    }

    /// Staleness is the document's own deadline, past-tense and absolute. It is never recomputed
    /// from how old the manifest looks, and never averaged into a verdict.
    @Test func stalenessIsTheDocumentsOwnDeadline() async throws {
        var builder = ManifestV2Builder()
        builder.staleAfter = Self.now.addingTimeInterval(-60)
        builder.nextGenerationExpectedAt = Self.now.addingTimeInterval(-120)
        let status = try await Self.client(builder).serviceStatus(now: Self.now)
        #expect(!status.isFresh(at: Self.now))
        #expect(status.staleAfter == Self.now.addingTimeInterval(-60))
        #expect(status.isFresh(at: Self.now.addingTimeInterval(-90)),
                "it was fresh right up to its deadline second")
    }

    /// An unreachable manifest throws rather than reporting an empty, healthy-looking service — the
    /// screen's "Unavailable" state exists precisely so this cannot be drawn as "no sources".
    @Test func anUnreachableManifestIsAnOutageNotAnEmptyServiceReport() async throws {
        var objects = try ManifestV2Builder().stubObjects()
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
        let status = try await Self.client(ManifestV2Builder()).serviceStatus(now: Self.now)
        #expect(status.frameCount == 2)
        #expect(status.latestFrameValidAt == Self.now.addingTimeInterval(900))
        #expect(status.referenceTime == Self.now.addingTimeInterval(-300))
        #expect(status.cellSizeMetres == 1_113)
        #expect(status.skippedFrames == 0)
    }
}
