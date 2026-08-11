import Foundation
import Testing
@testable import OBCWeather

/// Manifest v2 (WXR4 #1243, WXR5 #1244), against the **shared** fixture both clients read.
///
/// The Swift twin of `host/obc-wx-client/tests/manifest_v2.rs`, case for case. Until v2 only the
/// `.obcg`/`.obcw` byte vectors were cross-pinned and Swift synthesised its own manifests, so the two
/// parsers could drift on the one document every rider reads first. They cannot now: both suites read
/// `specs/vectors/wx-manifest-v2.json`.
///
/// The bbox cases are **not written here**. They live in `specs/vectors/manifest.json`'s
/// `wx_manifest_v2.bbox_equivalence` and this suite is a driver over them, so a case added for one
/// language is automatically a case the other must answer identically. That table is the whole
/// cross-client contract, and it is deliberately built out of the geometry a second implementer can
/// get wrong while passing everything else: an exact shard boundary, a southern-hemisphere corridor,
/// an antimeridian wrap, the polar band, and three bboxes that must be refused rather than clamped.
struct ManifestV2Tests {
    static func repositoryFile(_ relative: String) throws -> Data {
        let url = WeatherFixtures.repositoryRoot.appendingPathComponent(relative)
        return try #require(FileManager.default.contents(atPath: url.path), "missing \(relative)")
    }

    static func parsed() throws -> WeatherManifestV2 {
        try WeatherManifestV2.parse(repositoryFile("specs/vectors/wx-manifest-v2.json"))
    }

    /// Inside the fixture's generation, so nothing is expired unless a test says so.
    static let during = RFC3339.parse("2026-08-10T14:40:00Z")!

    static func around(
        latitudeMicrodegrees: Int64, longitudeMicrodegrees: Int64, spanMicrodegrees: Int64
    ) -> WeatherBoundingBox {
        WeatherBoundingBox(
            southMicrodegrees: latitudeMicrodegrees - spanMicrodegrees,
            westMicrodegrees: longitudeMicrodegrees - spanMicrodegrees,
            northMicrodegrees: latitudeMicrodegrees + spanMicrodegrees,
            eastMicrodegrees: longitudeMicrodegrees + spanMicrodegrees)
    }

    @Test
    func theSharedFixtureParsesAndStatesTheWholeLattice() throws {
        let manifest = try Self.parsed()
        #expect(manifest.generation == "20260810T1430Z")
        #expect(manifest.previousGenerations == ["20260810T1415Z", "20260810T1400Z"])
        #expect(manifest.skippedFrames == 0)
        #expect(manifest.frames.count == 9)

        // Nothing here is a client constant: the client reads the lattice it must address.
        let lattice = manifest.lattice
        #expect(lattice.width == 36_000 && lattice.height == 18_000)
        #expect(lattice.shardColumns == 6 && lattice.shardRows == 4 && lattice.shardCount == 24)
        #expect(lattice.shardWidth == 6_144 && lattice.shardHeight == 4_608)
        #expect(lattice.tileEdge == 256 && lattice.entriesPerPage == 128)
        #expect(lattice.cellSizeMetres == 1_113)
        #expect(lattice.coveredRows == 12..<17_987, "the polar band is stated once, not inferred")
        #expect(manifest.cadence.frameStepMinutes == 15)
        #expect(manifest.cadence.frames == 9)
        #expect(manifest.cadence.maximumSourceSkew == 1_800)
        // Every source that may have painted a cell; there is no per-cell provenance to narrow it to.
        #expect(manifest.attributions.map(\.sourceID) == ["dwd-rv", "us", "icon-eu", "gfs"])
    }

    /// **The test that replaces product selection**, driven from the cross-language table.
    ///
    /// Every case pins three things the two clients could otherwise get wrong independently: the
    /// shard set, the composed keys, and the plan's *outcome* — because "no objects" is three
    /// different answers and only one of them is about rain.
    @Test
    func everyPinnedBboxCaseAgreesWithTheSharedFixture() throws {
        let manifest = try Self.parsed()
        let vectors = try JSONSerialization.jsonObject(
            with: Self.repositoryFile("specs/vectors/manifest.json")) as! [String: Any]
        let block = try #require(vectors["wx_manifest_v2"] as? [String: Any])
        let cases = try #require(block["bbox_equivalence"] as? [[String: Any]])
        #expect(cases.count >= 10, "the table is the contract; do not shrink it")

        for testCase in cases {
            let name = try #require(testCase["name"] as? String)
            let box = try #require(testCase["bbox_udeg"] as? [String: Int64])
            let bbox = WeatherBoundingBox(
                southMicrodegrees: try #require(box["south"]),
                westMicrodegrees: try #require(box["west"]),
                northMicrodegrees: try #require(box["north"]),
                eastMicrodegrees: try #require(box["east"]))
            let expectedShards = (try #require(testCase["shards"] as? [[String: UInt32]])).map {
                WeatherShardID(column: $0["col"]!, row: $0["row"]!)
            }

            switch testCase["error"] as? String {
            case "out_of_range":
                #expect(throws: WeatherBboxError.outOfRange, "\(name)") {
                    try manifest.lattice.shards(for: bbox)
                }
            case .some(let other):
                Issue.record("\(name): unknown pinned error \(other)")
            case nil:
                let shards = try manifest.lattice.shards(for: bbox)
                #expect(shards == expectedShards, "\(name): shard set")
                let keys = expectedShards.map {
                    manifest.lattice.shardKey(offsetMinutes: 0, shard: $0)
                }
                #expect(keys == (try #require(testCase["f0_keys"] as? [String])),
                        "\(name): composed keys")
            }

            let plan = manifest.plan(bbox: bbox, now: Self.during)
            let expected: WeatherPlanOutcome
            switch try #require(testCase["outcome"] as? String) {
            case "covered": expected = .covered
            case "uncovered": expected = .uncovered
            case "out_of_domain": expected = .outOfDomain
            case let other:
                Issue.record("\(name): unknown pinned outcome \(other)")
                continue
            }
            #expect(plan.outcome == expected, "\(name): outcome")
            if expected != .covered {
                #expect(plan.fetch.isEmpty && plan.dry.isEmpty,
                        "\(name): only covered carries vectors")
            }
        }
    }

    /// The three-valued answer, which is the whole reason the bitmap exists: **a 404 must never mean
    /// dry**, and a dry shard must never look like a failure.
    @Test
    func missingIsNotDryAndDryIsNotMissing() throws {
        let manifest = try Self.parsed()
        let lattice = manifest.lattice
        let f0 = try #require(manifest.frame(offsetMinutes: 0))

        // Present: an object to fetch, with the integrity data to check it against.
        guard case let .present(key, byteLength, objectCRC32, observed) =
            f0.state(of: WeatherShardID(column: 3, row: 2), in: lattice)
        else {
            Issue.record("expected an object at s3-2")
            return
        }
        #expect(key == "wx/v2/20260810T1430Z/f0/s3-2.obcg")
        #expect(byteLength == 120_000 + 3 * 3_000 + 2 * 700)
        #expect(objectCRC32 == 0x51A0_0000 + 47 * 0x0001_0101)
        #expect(observed, "a shard painted end to end by radar says so, per shard, not per frame")

        // Dry: the baker measured every cell dry and published nothing. No request, no error.
        #expect(f0.state(of: WeatherShardID(column: 2, row: 0), in: lattice) == .dry)
        #expect(f0.state(of: WeatherShardID(column: 3, row: 0), in: lattice) == .dry)
        // ...and dryness is per frame: the same shard has an object at f15.
        let f15 = try #require(manifest.frame(offsetMinutes: 15))
        #expect(f15.state(of: WeatherShardID(column: 2, row: 0), in: lattice) != .dry)
        // ...and the last frame has its own hole.
        let f120 = try #require(manifest.frame(offsetMinutes: 120))
        #expect(f120.state(of: WeatherShardID(column: 5, row: 3), in: lattice) == .dry)
        #expect(f0.state(of: WeatherShardID(column: 5, row: 3), in: lattice) != .dry)

        // Off the lattice: geometry, not weather and not an error.
        #expect(f0.state(of: WeatherShardID(column: 6, row: 0), in: lattice) == .outOfDomain)
        #expect(f0.state(of: WeatherShardID(column: 0, row: 4), in: lattice) == .outOfDomain)

        // A whole-timeline plan over the f120 hole fetches eight objects and reports the ninth as
        // dry — the two ride different vectors, so neither can be rendered as the other.
        let plan = manifest.plan(
            bbox: Self.around(
                latitudeMicrodegrees: 85_000_000, longitudeMicrodegrees: 175_000_000,
                spanMicrodegrees: 100_000),
            now: Self.during)
        #expect(plan.outcome == .covered)
        #expect(plan.fetch.count == 8)
        #expect(plan.dry == [
            WeatherFrameShard(offsetMinutes: 120, shard: WeatherShardID(column: 5, row: 3)),
        ])
        #expect(plan.fetch.allSatisfy { $0.key.hasSuffix("s5-3.obcg") })
    }

    /// The bitmap and the shard list are two spellings of one fact, and the module holds them equal
    /// by keeping both private and looking the shard up in the list. Pinned across the whole fixture
    /// so the two can never answer differently for any shard of any frame.
    @Test
    func theBitmapAndTheLookupAreTheSameAnswer() throws {
        let manifest = try Self.parsed()
        let lattice = manifest.lattice
        var present = 0
        for frame in manifest.frames {
            for row in 0..<lattice.shardRows {
                for column in 0..<lattice.shardColumns {
                    let shard = WeatherShardID(column: column, row: row)
                    let byBitmap = frame.isPresent(shard, in: lattice)
                    var byLookup = false
                    if case .present = frame.state(of: shard, in: lattice) { byLookup = true }
                    #expect(byBitmap == byLookup, "f\(frame.offsetMinutes) s\(column)-\(row)")
                    if byBitmap { present += 1 }
                }
            }
        }
        #expect(zip(manifest.frames, manifest.frames).allSatisfy { frame, _ in
            zip(frame.shards, frame.shards.dropFirst()).allSatisfy { $0.id < $1.id }
        }, "ascending by (row, col)")
        #expect(present == 9 * 24 - 3, "the fixture's three deliberate holes")
    }

    /// **`(row, col)`, not `(col, row)`.** The document orders `shards[]` that way, `shards(for:)`
    /// returns that way, and the presence bit index `row * shardColumns + col` counts that way. The
    /// mismatch is not cosmetic: `state(of:in:)` binary-searches the shard list, so the wrong order
    /// makes it answer **dry for shards that exist**, which is the one answer this issue forbids.
    @Test
    func shardIDsSortByRowThenColumn() throws {
        var shards = [
            WeatherShardID(column: 5, row: 0), WeatherShardID(column: 0, row: 1),
            WeatherShardID(column: 1, row: 0), WeatherShardID(column: 0, row: 2),
            WeatherShardID(column: 2, row: 1), WeatherShardID(column: 0, row: 0),
            WeatherShardID(column: 5, row: 3), WeatherShardID(column: 1, row: 1),
        ]
        shards.sort()
        #expect(shards == [
            WeatherShardID(column: 0, row: 0), WeatherShardID(column: 1, row: 0),
            WeatherShardID(column: 5, row: 0), WeatherShardID(column: 0, row: 1),
            WeatherShardID(column: 1, row: 1), WeatherShardID(column: 2, row: 1),
            WeatherShardID(column: 0, row: 2), WeatherShardID(column: 5, row: 3),
        ], "rows first, then columns within a row")
        #expect(WeatherShardID(column: 5, row: 0) < WeatherShardID(column: 0, row: 1),
                "the last shard of row 0 precedes the first shard of row 1")
    }

    /// The bitmap and `shards[]` are one statement. A document where they disagree is not reconciled —
    /// either direction of reconciliation invents a fact about whether an object exists.
    @Test
    func aFrameWhoseBitmapAndListDisagreeIsSkippedRatherThanReconciled() throws {
        var document = try JSONSerialization.jsonObject(
            with: Self.repositoryFile("specs/vectors/wx-manifest-v2.json")) as! [String: Any]
        var frames = try #require(document["frames"] as? [[String: Any]])

        // A shard listed but not in the bitmap.
        var listed = frames
        var first = listed[0]
        var shards = try #require(first["shards"] as? [[String: Any]])
        shards.append([
            "col": 2, "row": 0, "bytes": 1_234, "object_crc32": "0x00000001", "observed": false,
        ])
        first["shards"] = shards
        listed[0] = first
        var mutated = document
        mutated["frames"] = listed
        var parsed = try WeatherManifestV2.parse(
            JSONSerialization.data(withJSONObject: mutated))
        #expect(parsed.skippedFrames == 1, "the frame is skipped and counted, never fatal")
        #expect(parsed.frames.count == 8)

        // A shard in the bitmap with no entry.
        var second = frames[1]
        var secondShards = try #require(second["shards"] as? [[String: Any]])
        secondShards.removeFirst()
        second["shards"] = secondShards
        frames[1] = second
        document["frames"] = frames
        parsed = try WeatherManifestV2.parse(JSONSerialization.data(withJSONObject: document))
        #expect(parsed.skippedFrames == 1)
        #expect(parsed.frame(offsetMinutes: 15) == nil)
    }

    /// The document is strict where being lenient would let a manifest steer the client, where a
    /// lattice it cannot address leaves nothing to degrade to, and where the client and the service's
    /// sweep would otherwise disagree about what exists.
    @Test
    func theDocumentIsStrictAboutVersionAddressingTheLatticeAndRetention() throws {
        let base = try JSONSerialization.jsonObject(
            with: Self.repositoryFile("specs/vectors/wx-manifest-v2.json")) as! [String: Any]

        var wrongVersion = base
        wrongVersion["version"] = 1
        #expect(throws: WeatherManifestError.unsupportedVersion(1)) {
            try WeatherManifestV2.parse(JSONSerialization.data(withJSONObject: wrongVersion))
        }

        let mutations: [(String, [String], Any)] = [
            ("a traversing key prefix", ["key_prefix"], "../../etc"),
            ("an absolute key prefix", ["key_prefix"], "/wx/v2"),
            ("a traversing generation", ["generation"], "20260810T1430Z/../.."),
            // Three generations is the client and the sweep disagreeing about what exists.
            ("an over-long retention chain", ["previous_generations"],
             ["20260810T1415Z", "20260810T1400Z", "20260810T1345Z"]),
            // The shard grid must be the one that tiles the lattice.
            ("a shard grid that does not tile", ["lattice", "shard_cols"], 7),
            // A shard no OBCG header could express is not worth a Range read.
            ("a shard too wide for an OBCG header", ["lattice", "shard_width"], 36_000),
            ("covered_rows past the lattice", ["lattice", "covered_rows", "end"], 18_001),
            // A cadence that disagrees with its own frame list is a mis-derived cycle.
            ("a cadence short of its frame list", ["cadence", "frames"], 8),
            // A generation that expires before its replacement is due.
            ("a deadline before the next generation", ["freshness", "stale_after"],
             "2026-08-10T14:40:00Z"),
        ]
        for (why, path, value) in mutations {
            var broken = base
            replace(&broken, path: path, with: value)
            #expect(throws: WeatherManifestError.malformed, "\(why) must be refused") {
                try WeatherManifestV2.parse(JSONSerialization.data(withJSONObject: broken))
            }
        }

        // Two frames naming the same object at two validities.
        var duplicate = base
        var frames = try #require(duplicate["frames"] as? [[String: Any]])
        var clone = frames[1]
        clone["offset_min"] = 0
        frames.insert(clone, at: 1)
        duplicate["frames"] = frames
        replace(&duplicate, path: ["cadence", "frames"], with: 10)
        #expect(throws: WeatherManifestError.malformed, "duplicate offset_min") {
            try WeatherManifestV2.parse(JSONSerialization.data(withJSONObject: duplicate))
        }
    }

    /// Every deadline is read, not held. The client compares timestamps against the document, so the
    /// service can change the cadence without a client release.
    @Test
    func theDeadlinesComeFromTheDocumentNotFromAClientConstant() throws {
        let manifest = try Self.parsed()
        let staleAfter = try #require(RFC3339.parse("2026-08-10T16:30:00Z"))
        #expect(manifest.freshness.staleAfter == staleAfter)
        #expect(manifest.freshness.nextGenerationExpectedAt
            == (try #require(RFC3339.parse("2026-08-10T14:45:00Z"))))
        #expect(manifest.freshness.isUsable(at: staleAfter), "inclusive to the last second")
        #expect(!manifest.freshness.isUsable(at: staleAfter.addingTimeInterval(1)))
        #expect(manifest.freshness.manifestMaximumAge == 60)
        let fetchedAt = manifest.generatedAt
        #expect(!manifest.freshness.manifestIsStale(
            fetchedAt: fetchedAt, now: fetchedAt.addingTimeInterval(60)))
        #expect(manifest.freshness.manifestIsStale(
            fetchedAt: fetchedAt, now: fetchedAt.addingTimeInterval(61)))
    }

    /// **Expiry is no weather, and no weather is not a dry map.** The check lives inside `plan`
    /// rather than in a caller's discipline, because "did anyone remember to check the deadline
    /// first" is exactly the contract that holds until the one call site that forgets — and the thing
    /// it would render is the forbidden one.
    @Test
    func anExpiredGenerationIsNoWeatherNotADryMap() throws {
        let manifest = try Self.parsed()
        let freiburg = Self.around(
            latitudeMicrodegrees: 48_000_000, longitudeMicrodegrees: 7_850_000,
            spanMicrodegrees: 100_000)

        let live = manifest.plan(bbox: freiburg, now: Self.during)
        #expect(live.outcome == .covered)
        #expect(live.fetch.count == 9)

        let expired = manifest.plan(
            bbox: freiburg, now: manifest.freshness.staleAfter.addingTimeInterval(1))
        #expect(expired.outcome == .expired)
        #expect(expired.fetch.isEmpty && expired.dry.isEmpty)
        // The frames are still there and still true; what expired is the right to answer with them.
        let f0 = try #require(manifest.frame(offsetMinutes: 0))
        if case .present = f0.state(of: WeatherShardID(column: 3, row: 2), in: manifest.lattice) {
        } else {
            Issue.record("the frame's own state is unchanged by the deadline")
        }
    }

    /// **The unclamped intersection, pinned where it is observable.**
    ///
    /// On the global lattice this fix hides: every in-range coordinate is on the lattice, so
    /// ``WeatherBoundingBox/validateAsWindow()`` catches the off-map cases before the arithmetic ever
    /// runs, and clamp-first and intersect-first agree on everything left. A **regional** lattice is
    /// the only geometry where they disagree — and the manifest states the lattice, so a regional one
    /// is a baker deploy away rather than hypothetical.
    @Test
    func anOffLatticeBboxVanishesRatherThanCollapsingOntoTheEdge() throws {
        let lattice = WeatherLattice(
            southLatitudeMicrodegrees: 47_000_000, westLongitudeMicrodegrees: 5_000_000,
            cellMicrodegrees: 10_000, width: 1_000, height: 600, shardWidth: 500,
            shardHeight: 300, shardColumns: 2, shardRows: 2, tileEdge: 256, entriesPerPage: 128,
            cellSizeMetres: 1_113, coveredRows: 0..<600, keyPrefix: "wx/v2",
            generation: "20260810T1430Z")

        // Inside: the control, so a lattice that answered nothing to everything cannot pass.
        let inside = WeatherBoundingBox(
            southMicrodegrees: 48_000_000, westMicrodegrees: 7_750_000,
            northMicrodegrees: 48_100_000, eastMicrodegrees: 7_950_000)
        #expect(try lattice.shards(for: inside) == [WeatherShardID(column: 0, row: 0)])

        // Every direction of "wholly outside, but a perfectly legal coordinate pair". Clamp-first
        // returns the nearest edge shard for each — the rider is handed a neighbouring region's
        // weather instead of being told they are off the map. Intersect-first returns nothing.
        let outside: [(String, WeatherBoundingBox)] = [
            ("west of it", WeatherBoundingBox(
                southMicrodegrees: 48_000_000, westMicrodegrees: 0,
                northMicrodegrees: 48_100_000, eastMicrodegrees: 1_000_000)),
            ("east of it", WeatherBoundingBox(
                southMicrodegrees: 48_000_000, westMicrodegrees: 20_000_000,
                northMicrodegrees: 48_100_000, eastMicrodegrees: 21_000_000)),
            ("south of it", WeatherBoundingBox(
                southMicrodegrees: 40_000_000, westMicrodegrees: 7_750_000,
                northMicrodegrees: 41_000_000, eastMicrodegrees: 7_950_000)),
            ("north of it", WeatherBoundingBox(
                southMicrodegrees: 60_000_000, westMicrodegrees: 7_750_000,
                northMicrodegrees: 61_000_000, eastMicrodegrees: 7_950_000)),
            ("diagonally past the corner", WeatherBoundingBox(
                southMicrodegrees: 60_000_000, westMicrodegrees: 20_000_000,
                northMicrodegrees: 61_000_000, eastMicrodegrees: 21_000_000)),
        ]
        for (where_, bbox) in outside {
            #expect(try lattice.shards(for: bbox).isEmpty,
                    "\(where_): an off-lattice bbox must vanish, not collapse onto the edge shard")
        }

        // Exactly abutting the west edge from outside is still outside: the window is half-open, so
        // an east edge on the lattice origin closes the cell before it, which is not a cell here.
        let abutting = WeatherBoundingBox(
            southMicrodegrees: 48_000_000, westMicrodegrees: 4_000_000,
            northMicrodegrees: 48_100_000, eastMicrodegrees: 5_000_000)
        #expect(try lattice.shards(for: abutting).isEmpty)
        // One microdegree further east and it touches the first cell.
        let touching = WeatherBoundingBox(
            southMicrodegrees: 48_000_000, westMicrodegrees: 4_000_000,
            northMicrodegrees: 48_100_000, eastMicrodegrees: 5_000_001)
        #expect(try lattice.shards(for: touching) == [WeatherShardID(column: 0, row: 0)])
    }

    /// Shard geometry is derived, not published — and the last column and row are **short**.
    ///
    /// The lattice need not be a whole number of shards wide, so assuming a full square at the edge
    /// is how a client reads a neighbouring shard's bytes as this shard's north edge, or refuses a
    /// perfectly good header for disagreeing with a width nobody publishes.
    @Test
    func edgeShardsAreShortAndTheirGeometryIsDerived() throws {
        let manifest = try Self.parsed()
        let lattice = manifest.lattice

        let interior = try #require(lattice.geometry(of: WeatherShardID(column: 3, row: 2)))
        #expect(interior.width == 6_144 && interior.height == 4_608)
        #expect(interior.westMicrodegrees == -180_000_000 + 3 * 6_144 * 10_000)
        #expect(interior.southMicrodegrees == -90_000_000 + 2 * 4_608 * 10_000)
        #expect(interior.cellLatitudeMicrodegrees == 10_000)
        #expect(interior.cellLongitudeMicrodegrees == 10_000)
        #expect(interior.cellSizeMetres == 1_113)

        // 36,000 / 6,144 = 5 full columns plus 5,280; 18,000 / 4,608 = 3 full rows plus 4,176.
        let corner = try #require(lattice.geometry(of: WeatherShardID(column: 5, row: 3)))
        #expect(corner.width == 36_000 - 5 * 6_144)
        #expect(corner.height == 18_000 - 3 * 4_608)
        #expect(corner.bounds.eastMicrodegrees == 180_000_000)
        #expect(corner.bounds.northMicrodegrees == 90_000_000)

        // A shard off the lattice has no geometry, and asking is not a crash. Without the guard
        // `width - columnOrigin` underflows and Swift traps — reachable through `bounds(of:)`,
        // which is public. Rust's `shard_geometry` opens with the same `bit_of(shard)?`.
        #expect(lattice.geometry(of: WeatherShardID(column: 6, row: 0)) == nil)
        #expect(lattice.geometry(of: WeatherShardID(column: 0, row: 4)) == nil)
        #expect(lattice.bounds(of: WeatherShardID(column: 99, row: 99)) == nil)
    }

    /// Two entries for one shard: **which one survives is the document's order, not the sort's.**
    ///
    /// Rust's `sort_by_key` is stable and its `dedup_by_key` keeps the first of a run, so a
    /// duplicated shard resolves to the entry written first. Swift's `Array.sort` is not stable, so
    /// without an index tiebreak the two clients would disagree about a shard's `bytes` and
    /// `object_crc32` — one of them then refusing an object the other fetches.
    @Test
    func aDuplicatedShardEntryResolvesToTheFirstOneTheDocumentWrote() throws {
        var document = try JSONSerialization.jsonObject(
            with: Self.repositoryFile("specs/vectors/wx-manifest-v2.json")) as! [String: Any]
        var frames = try #require(document["frames"] as? [[String: Any]])
        var first = frames[0]
        var shards = try #require(first["shards"] as? [[String: Any]])
        // Repeat the frame's first shard with different integrity data. The bitmap still names it
        // once, so the popcount check still passes and the frame survives.
        var repeated = shards[0]
        repeated["bytes"] = 4_242
        repeated["object_crc32"] = "0xdeadbeef"
        shards.append(repeated)
        first["shards"] = shards
        frames[0] = first
        document["frames"] = frames

        let manifest = try WeatherManifestV2.parse(
            JSONSerialization.data(withJSONObject: document))
        #expect(manifest.skippedFrames == 0)
        let original = try #require(shards[0]["bytes"] as? Int)
        let f0 = try #require(manifest.frame(offsetMinutes: 0))
        let shard = WeatherShardID(
            column: try #require(shards[0]["col"] as? UInt32),
            row: try #require(shards[0]["row"] as? UInt32))
        guard case let .present(_, byteLength, _, _) = f0.state(of: shard, in: manifest.lattice)
        else {
            Issue.record("the duplicated shard must still be present")
            return
        }
        #expect(byteLength == original, "the first entry the document wrote is the one that survives")
    }

    // MARK: - The shared rejection corpus

    /// **The other half of the cross-client contract.** `bbox_equivalence` pins what the two clients
    /// compute; this pins what they *refuse* — and the review that asked for it found five documents
    /// the two answered differently and two that crashed this one outright.
    ///
    /// A manifest is the first thing a rider's phone fetches from a network nobody controls, so
    /// "does not crash" is the floor and "answers identically" is the contract. Every case is a
    /// mutation of the shared fixture, and the verdicts live in the fixture directory rather than
    /// here, so this suite cannot quietly diverge from
    /// `host/obc-wx-client/tests/manifest_v2.rs::the_shared_rejection_corpus_is_answered_identically`
    /// by testing a different list.
    @Test
    func theSharedRejectionCorpusIsAnsweredIdentically() throws {
        let directory = try JSONSerialization.jsonObject(
            with: Self.repositoryFile("specs/vectors/manifest.json")) as! [String: Any]
        let block = try #require(directory["wx_manifest_v2"] as? [String: Any])
        let corpus = try #require(block["rejection_equivalence"] as? [String: Any])
        let cases = try #require(corpus["cases"] as? [[String: Any]])
        #expect(cases.count >= 20,
                "the corpus must not shrink; it is the only pin on rejection parity")
        let base = try JSONSerialization.jsonObject(
            with: Self.repositoryFile("specs/vectors/wx-manifest-v2.json"))

        for testCase in cases {
            let name = try #require(testCase["name"] as? String)
            var document = base
            for operation in try #require(testCase["patch"] as? [[String: Any]]) {
                let pointer = try #require(operation["path"] as? String)
                let steps = pointer.split(separator: "/", omittingEmptySubsequences: false)
                    .dropFirst().map(String.init)
                document = Self.patched(
                    document, path: steps[...],
                    operation: try #require(operation["op"] as? String),
                    value: operation["value"] ?? NSNull())
            }
            let bytes = Data(Self.serialised(document).utf8)
            // A pointer the walker silently failed to follow would leave the happy path behind and
            // "pass" every rejected case for the wrong reason. Every case mutates the document.
            #expect(bytes != Data(Self.serialised(base).utf8), "\(name): the patch changed nothing")

            switch try #require(testCase["verdict"] as? String) {
            case "rejected":
                #expect(throws: (any Error).self, "\(name): must be refused, parsed instead") {
                    try WeatherManifestV2.parse(bytes)
                }
            case "accepted":
                do {
                    let manifest = try WeatherManifestV2.parse(bytes)
                    #expect(manifest.skippedFrames == (testCase["skipped_frames"] as? Int) ?? 0,
                            "\(name): skipped frames")
                } catch {
                    Issue.record("\(name): must parse, got \(error)")
                }
            case let other:
                Issue.record("\(name): unknown verdict \(other)")
            }
        }
    }

    /// Apply one corpus patch op to a decoded JSON tree. RFC 6901 pointers, `set` and `remove`.
    ///
    /// Deliberately tiny and deliberately hand-rolled rather than a JSON-Patch package: the whole
    /// value of the corpus is that two *independent* implementations answer the same way, and a
    /// library on this side against Rust's own hand walker on the other is exactly the asymmetry
    /// that would make a "cross-language" fixture test one language.
    static func patched(
        _ node: Any, path: ArraySlice<String>, operation: String, value: Any
    ) -> Any {
        guard let step = path.first else { return node }
        let rest = path.dropFirst()
        if var object = node as? [String: Any] {
            if rest.isEmpty {
                if operation == "remove" { object.removeValue(forKey: step) } else {
                    object[step] = value
                }
            } else if let child = object[step] {
                object[step] = patched(child, path: rest, operation: operation, value: value)
            }
            return object
        }
        if var array = node as? [Any], let index = Int(step), array.indices.contains(index) {
            if rest.isEmpty {
                if operation == "remove" { array.remove(at: index) } else { array[index] = value }
            } else {
                array[index] = patched(
                    array[index], path: rest, operation: operation, value: value)
            }
            return array
        }
        return node
    }

    /// A minimal JSON writer, because `JSONSerialization` **would erase the point of two cases**: it
    /// re-serialises the `Double` 2.0 as `2`, which turns `version-is-a-float` into a document
    /// indistinguishable from the happy path and makes the case vacuous. serde_json writes `2.0`, so
    /// this writes `2.0` — the float-ness of a wire number is exactly what the corpus is about.
    static func serialised(_ node: Any) -> String {
        if node is NSNull { return "null" }
        if let number = node as? NSNumber {
            if CFGetTypeID(number) == CFBooleanGetTypeID() {
                return number.boolValue ? "true" : "false"
            }
            return CFNumberIsFloatType(number)
                ? String(number.doubleValue) : String(number.int64Value)
        }
        if let text = node as? String { return quoted(text) }
        if let array = node as? [Any] {
            return "[" + array.map(serialised).joined(separator: ",") + "]"
        }
        if let object = node as? [String: Any] {
            // Sorted so a failure is reproducible; JSON object order carries no meaning.
            return "{" + object.keys.sorted().map { "\(quoted($0)):\(serialised(object[$0]!))" }
                .joined(separator: ",") + "}"
        }
        return "null"
    }

    private static func quoted(_ text: String) -> String {
        var out = "\""
        for character in text.unicodeScalars {
            switch character {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            case let scalar where scalar.value < 0x20:
                out += String(format: "\\u%04x", scalar.value)
            // Everything else goes out as itself, including non-ASCII: the corpus feeds this a
            // multi-byte `present` string on purpose, and escaping it would test a different one.
            case let scalar: out.unicodeScalars.append(scalar)
            }
        }
        return out + "\""
    }

    private func replace(_ document: inout [String: Any], path: [String], with value: Any) {
        guard let head = path.first else { return }
        if path.count == 1 {
            document[head] = value
            return
        }
        guard var nested = document[head] as? [String: Any] else { return }
        replace(&nested, path: Array(path.dropFirst()), with: value)
        document[head] = nested
    }
}
