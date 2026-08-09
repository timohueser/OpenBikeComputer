import Foundation
import Testing
@testable import OBCWeather

/// Tier selection is a pure function of manifest data. Every case below is expressed by *changing
/// the fixture manifest*, never by changing code — which is the property the epic actually cares
/// about: adding a region or a source must be a baker deploy.
struct ProductSelectionTests {
    static let now = Date(timeIntervalSince1970: 1_800_000_000)
    /// Well inside every fixture product's window — including the 32 x 32-cell floor object, which
    /// is the smallest of the three.
    static let corridor = WeatherCorridor(
        bounds: WeatherBoundingBox(
            southMicrodegrees: 47_180_000, westMicrodegrees: 7_280_000,
            northMicrodegrees: 47_287_000, eastMicrodegrees: 7_447_000),
        isUndirected: true)

    /// Radar / model / floor over the same corridor, each with its own deadline.
    static func manifest(
        radarDeadline: TimeInterval, modelDeadline: TimeInterval, floorDeadline: TimeInterval
    ) throws -> WeatherServiceManifest {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "dwd-rv", tier: 1, vectors: ["grid-multipage.obcg"],
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(radarDeadline)))
        try builder.add(ManifestBuilder.ProductSpec(
            id: "icon-eu", tier: 2, vectors: ["grid-raw-tile.obcg"],
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(modelDeadline)))
        try builder.add(ManifestBuilder.ProductSpec(
            id: "gfs", tier: 3, vectors: ["grid-minimal-dry.obcg"],
            referenceTime: now.addingTimeInterval(-300), generatedAt: now,
            stalenessDeadline: now.addingTimeInterval(floorDeadline)))
        return try WeatherServiceManifest.parse(builder.json()).manifest
    }

    private func selectedID(_ manifest: WeatherServiceManifest, now: Date = now) -> String? {
        guard case let .selected(product) = ProductSelection.select(
            from: manifest, corridor: Self.corridor, now: now).outcome else { return nil }
        return product.id
    }

    @Test
    func theHighestFreshTierCoveringTheCorridorWins() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        #expect(selectedID(manifest) == "dwd-rv")
    }

    @Test
    func anExpiredRadarProductFallsBackToTheModelThenToTheFloor() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        // Past the radar deadline only: the model answers.
        #expect(selectedID(manifest, now: Self.now.addingTimeInterval(1_200)) == "icon-eu")
        // Past the model deadline too: the worldwide floor answers.
        #expect(selectedID(manifest, now: Self.now.addingTimeInterval(4_000)) == "gfs")
    }

    @Test
    func aFullyStaleManifestYieldsHourlyOnlyAndNamesWhatExpired() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        let result = ProductSelection.select(
            from: manifest, corridor: Self.corridor, now: Self.now.addingTimeInterval(30_000))
        #expect(result.outcome == .none(
            .allCoveringProductsExpired(latestDeadline: Self.now.addingTimeInterval(21_600))))
        // Expired products are *reported*, not silently dropped — WX13 can say why the map is gone.
        #expect(Set(result.expired) == ["dwd-rv", "icon-eu", "gfs"])
    }

    @Test
    func aCorridorNoProductCoversYieldsTheExplicitNoRainMapState() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        let gap = WeatherCorridor(
            bounds: WeatherBoundingBox(
                southMicrodegrees: -33_900_000, westMicrodegrees: 18_400_000,
                northMicrodegrees: -33_800_000, eastMicrodegrees: 18_500_000),
            isUndirected: true)
        let result = ProductSelection.select(from: manifest, corridor: gap, now: Self.now)
        #expect(result.outcome == .none(.corridorNotCovered))
        #expect(result.expired.isEmpty)
    }

    /// A corridor that only *overlaps* a product is not covered by it. Containment is the rule
    /// because a rain map that silently stops halfway down the corridor is worse than none.
    @Test
    func partialOverlapDoesNotCount() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        let straddling = WeatherCorridor(
            bounds: WeatherBoundingBox(
                southMicrodegrees: 46_900_000, westMicrodegrees: 7_100_000,
                northMicrodegrees: 47_100_000, eastMicrodegrees: 7_300_000),
            isUndirected: true)
        // Every fixture product starts at 47.0 N, so a corridor reaching to 46.9 is covered by none.
        #expect(ProductSelection.select(
            from: manifest, corridor: straddling, now: Self.now).outcome == .none(.corridorNotCovered))
    }

    /// The whole architecture in one test: a product this build has never heard of, from a region
    /// nobody wrote code for, is selected purely because the manifest says it covers the corridor
    /// at a better tier. No app release, no country check, no id allow-list.
    @Test
    func anUnknownProductIsSelectedWithNoAppChanges() throws {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "gfs", tier: 3, vectors: ["grid-minimal-dry.obcg"],
            referenceTime: Self.now.addingTimeInterval(-300), generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(21_600)))
        try builder.add(ManifestBuilder.ProductSpec(
            id: "inca-at", tier: 1, vectors: ["grid-multipage.obcg"],
            referenceTime: Self.now.addingTimeInterval(-60), generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(900),
            attributionText: "Source: GeoSphere Austria",
            attributionURL: "https://creativecommons.org/licenses/by/4.0/"))
        let manifest = try WeatherServiceManifest.parse(builder.json()).manifest
        guard case let .selected(product) = ProductSelection.select(
            from: manifest, corridor: Self.corridor, now: Self.now).outcome else {
            Issue.record("nothing selected"); return
        }
        #expect(product.id == "inca-at")
        #expect(product.attribution.text == "Source: GeoSphere Austria")
    }

    /// Two equally good products must always resolve the same way, or the same corridor would
    /// produce different bundles from one refresh to the next.
    @Test
    func tiesBreakOnTheNewerRunThenTheIdentifier() throws {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "b-older", tier: 1, vectors: ["grid-multipage.obcg"],
            referenceTime: Self.now.addingTimeInterval(-900), generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(900)))
        try builder.add(ManifestBuilder.ProductSpec(
            id: "a-newer", tier: 1, vectors: ["grid-multipage.obcg"],
            referenceTime: Self.now.addingTimeInterval(-300), generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(900)))
        let manifest = try WeatherServiceManifest.parse(builder.json()).manifest
        #expect(selectedID(manifest) == "a-newer")
    }

    @Test
    func onlyFramesInsideTheTwoHourWindowAreFetched() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        let model = try #require(manifest.products.first { $0.id == "icon-eu" })
        // The model frame is valid at +1 h: inside a two-hour question.
        #expect(ProductSelection.frames(of: model, now: Self.now).count == 1)
        // A day later it is neither a usable forecast nor a genuine recent observation.
        #expect(ProductSelection.frames(of: model, now: Self.now.addingTimeInterval(86_400)).isEmpty)
        // A genuinely latent observation keeps its old timestamp and stays usable.
        let radar = try #require(manifest.products.first { $0.id == "dwd-rv" })
        #expect(ProductSelection.frames(of: radar, now: Self.now.addingTimeInterval(3 * 3_600)).count == 1)
        #expect(ProductSelection.frames(of: radar, now: Self.now.addingTimeInterval(9 * 3_600)).isEmpty)
    }

    // MARK: - Manifest parsing

    @Test
    func aMalformedProductIsSkippedWhileTheRestOfTheManifestStillWorks() throws {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "gfs", tier: 3, vectors: ["grid-minimal-dry.obcg"],
            referenceTime: Self.now.addingTimeInterval(-300), generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(21_600)))
        // Tier 0 is the OBCG registry's "invalid"; the entry is otherwise well-formed JSON.
        builder.rawProducts = [[
            "id": "broken", "tier": 0,
            "bbox_udeg": [
                "south_udeg": 47_000_000, "west_udeg": 7_000_000,
                "north_udeg": 48_000_000, "east_udeg": 8_000_000,
            ],
            "cell": ["lat_udeg": 9_000, "lon_udeg": 14_000, "nominal_m": 1_000],
            "reference_time": "2027-01-15T12:00:00Z", "generated_at": "2027-01-15T12:00:00Z",
            "staleness_deadline": "2027-01-15T13:00:00Z",
            "attribution": ["text": "x", "url": "y"], "frames": [],
        ]]
        let parsed = try WeatherServiceManifest.parse(builder.json())
        #expect(parsed.skippedProducts == 1)
        #expect(parsed.manifest.products.map(\.id) == ["gfs"])
        #expect(selectedID(parsed.manifest) == "gfs")
    }

    @Test
    func anUnknownDocumentVersionIsAnOutageRatherThanAGuess() throws {
        var builder = ManifestBuilder()
        builder.version = 2
        try builder.add(ManifestBuilder.ProductSpec(
            id: "gfs", tier: 3, vectors: ["grid-minimal-dry.obcg"],
            referenceTime: Self.now, generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(21_600)))
        #expect(throws: WeatherManifestError.unsupportedVersion(2)) {
            try WeatherServiceManifest.parse(builder.json())
        }
        #expect(throws: WeatherManifestError.malformed) {
            try WeatherServiceManifest.parse(Data("{not json".utf8))
        }
    }

    @Test
    func extraUnknownFieldsAreToleratedSoTheBakerCanAddThem() throws {
        var builder = ManifestBuilder()
        try builder.add(ManifestBuilder.ProductSpec(
            id: "gfs", tier: 3, vectors: ["grid-minimal-dry.obcg"],
            referenceTime: Self.now, generatedAt: Self.now,
            stalenessDeadline: Self.now.addingTimeInterval(21_600)))
        var document = try JSONSerialization.jsonObject(with: builder.json()) as! [String: Any]
        document["future_field"] = ["anything": 1]
        var products = document["products"] as! [[String: Any]]
        products[0]["upstream_etag"] = "\"abc\""
        products[0]["future_product_field"] = 42
        document["products"] = products
        let parsed = try WeatherServiceManifest.parse(
            try JSONSerialization.data(withJSONObject: document))
        #expect(parsed.manifest.products.count == 1)
        #expect(parsed.skippedProducts == 0)
    }

    @Test
    func clockSkewIsReportedRatherThanCompensatedFor() throws {
        let manifest = try Self.manifest(
            radarDeadline: 900, modelDeadline: 3_600, floorDeadline: 21_600)
        #expect(!ProductSelection.clockSkewSuspected(manifest: manifest, now: Self.now))
        // A device whose clock lags the manifest by an hour still selects — the deadlines are all
        // manifest time — but the state says the arithmetic is not to be trusted.
        #expect(ProductSelection.clockSkewSuspected(
            manifest: manifest, now: Self.now.addingTimeInterval(-3_600)))
    }
}
