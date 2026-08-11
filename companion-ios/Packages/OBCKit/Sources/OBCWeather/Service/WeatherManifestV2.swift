import Foundation
import OBCWeatherWire

/// The phone's view of `wx/v2/manifest.json` (OBCG §10) — **selection as arithmetic** (#1244).
///
/// The Swift twin of `host/obc-wx-client/src/manifest_v2.rs`, and deliberately a line-for-line one:
/// the two clients are mirrors, they are pinned against the same fixture
/// (`specs/vectors/wx-manifest-v2.json` plus the `bbox_equivalence` table in
/// `specs/vectors/manifest.json`), and a difference between them is a bug in whichever one differs.
///
/// v1's reader existed to *choose*: it ranked products by tier, tested bbox containment and compared
/// staleness deadlines. None of that survives, because there is nothing to choose between — one
/// dataset, one lattice, one generation. This module answers exactly two questions, and neither is a
/// policy:
///
/// 1. **which shards cover my bbox** — ``WeatherLattice/shards(for:)``, a handful of divisions;
/// 2. **what is at that shard** — ``WeatherManifestFrame/state(of:in:)``: an object to fetch, a dry
///    shard, or a shard off the lattice entirely.
///
/// ``WeatherManifestV2/plan(bbox:now:)`` is those two joined for a whole timeline, and it is the
/// function the fetch path calls. It returns a ``WeatherPlanOutcome`` rather than a bare list,
/// because "no objects to fetch" is four different sentences to a rider — no rain, off the map, no
/// source here ever, or no weather at all because the generation expired — and only the first is
/// about rain. An empty array cannot say which, so it is not what this module hands back.
///
/// The object key is composed, never read: ``WeatherLattice/shardKey(offsetMinutes:shard:)`` builds
/// `<key_prefix>/<generation>/f<offset>/s<col>-<row>.obcg`, which is why the manifest does not carry
/// 216 key strings.
///
/// ## Missing is not dry
///
/// ``WeatherShardState`` is deliberately three-valued, and that is the whole point of the presence
/// bitmap:
///
/// - ``WeatherShardState/present(key:byteLength:objectCRC32:observed:)`` — the object exists. A 404,
///   a short body or a CRC mismatch is an **error** to retry and then surface, never an absence of
///   rain. The manifest is the integrity anchor: `bytes` and `object_crc32` are checked against what
///   comes back.
/// - ``WeatherShardState/dry`` — the baker measured every cell of that shard as dry and published
///   nothing. There is no request to make and no failure to report; the cells are painted intensity
///   0, because a dry frame is a real, all-zero frame rather than an absent one.
/// - ``WeatherShardState/outOfDomain`` — the bbox reaches off the lattice. Not weather, not an
///   error: geometry.
///
/// A shard that is entirely **no-data** is `present` with an object full of intensity 15, because
/// "we do not know" is data the rider is owed. Only genuinely dry shards are absent.
///
/// Strictness splits the way the phone already split it for v1, and for the same reason: the
/// document is strict (bad JSON, an unknown `version` or an unusable lattice is a hard failure), an
/// entry is lenient (a malformed frame is skipped and counted, never fatal).
public struct WeatherManifestV2: Equatable, Sendable {
    /// The one key the whole dataset hangs off (OBCG §10).
    public static let manifestKey = "wx/v2/manifest.json"
    /// The one document version this build understands. A different one is an outage, not something
    /// to guess at.
    public static let supportedVersion = 2
    /// OBCG §10.4's retention cap, mirrored here because the reader enforces it.
    public static let retainedPreviousGenerations = 2
    /// The most shards a document may claim. The production layout is 24 (a 6 x 4 grid); this is
    /// three orders of magnitude above it and bounds both the presence bitmap (8 KiB of hex) and
    /// every `UInt32` the shard arithmetic multiplies. It exists because `width` and `shard_width`
    /// arrive from a network and their quotient is otherwise unbounded — the Swift twin of
    /// `manifest_v2::MAX_SHARDS`.
    public static let maximumShards: UInt32 = 65_536
    /// How far the device clock may lead the manifest before freshness arithmetic stops being
    /// trustworthy. Dataset-level, so it survived the deletion of product selection. Beyond it the
    /// client still answers (the document's own deadlines are intact) but says so in diagnostics
    /// rather than silently trusting or silently discarding the dataset.
    public static let clockSkewTolerance: TimeInterval = 15 * 60

    public var generation: String
    public var generatedAt: Date
    public var referenceTime: Date
    /// Superseded generations still fetchable, newest first.
    public var previousGenerations: [String]
    public var lattice: WeatherLattice
    public var cadence: WeatherCadence
    public var freshness: WeatherFreshness
    public var attributions: [WeatherAttribution]
    public var frames: [WeatherManifestFrame]
    /// Frames the parser refused. Evidence for the diagnostics panel, never control flow.
    public var skippedFrames: Int

    public init(
        generation: String, generatedAt: Date, referenceTime: Date, previousGenerations: [String],
        lattice: WeatherLattice, cadence: WeatherCadence, freshness: WeatherFreshness,
        attributions: [WeatherAttribution], frames: [WeatherManifestFrame], skippedFrames: Int
    ) {
        self.generation = generation
        self.generatedAt = generatedAt
        self.referenceTime = referenceTime
        self.previousGenerations = previousGenerations
        self.lattice = lattice
        self.cadence = cadence
        self.freshness = freshness
        self.attributions = attributions
        self.frames = frames
        self.skippedFrames = skippedFrames
    }

    public func frame(offsetMinutes: UInt32) -> WeatherManifestFrame? {
        frames.first { $0.offsetMinutes == offsetMinutes }
    }

    /// True when the manifest claims to have been produced meaningfully after this device thinks
    /// "now" is. Reported, never compensated for.
    public func clockSkewSuspected(at now: Date) -> Bool {
        generatedAt.timeIntervalSince(now) > Self.clockSkewTolerance
    }

    /// What this client should do to cover `bbox` at `now`, across the whole timeline.
    ///
    /// **The fetch path's contract.** Read ``WeatherPlan/outcome`` first and the vectors second:
    /// outside ``WeatherPlanOutcome/covered`` both vectors are empty *and mean nothing*, and
    /// rendering that as a dry map is the failure this whole issue exists to make impossible. Inside
    /// `covered`, `fetch` names objects that MUST exist — a 404, a short body or a CRC mismatch is
    /// an error to retry and then surface — and `dry` names shards the baker measured as dry, which
    /// need no request, report no failure, and are painted as intensity 0.
    ///
    /// Expiry is checked here rather than left to a caller's discipline, because "did anyone
    /// remember to check the deadline first" is exactly the kind of contract that holds until the
    /// one call site that forgets — and the thing that call site would render is the forbidden one.
    public func plan(bbox: WeatherBoundingBox, now: Date) -> WeatherPlan {
        func empty(_ outcome: WeatherPlanOutcome) -> WeatherPlan {
            WeatherPlan(outcome: outcome, fetch: [], dry: [])
        }
        guard freshness.isUsable(at: now) else { return empty(.expired) }
        guard let shards = try? lattice.shards(for: bbox), !shards.isEmpty else {
            return empty(.outOfDomain)
        }
        guard lattice.anyRowHasASource(in: bbox) else { return empty(.uncovered) }

        var fetch: [WeatherPlannedRead] = []
        var dry: [WeatherFrameShard] = []
        for frame in frames {
            for shard in shards {
                switch frame.state(of: shard, in: lattice) {
                case let .present(key, byteLength, objectCRC32, observed):
                    fetch.append(WeatherPlannedRead(
                        offsetMinutes: frame.offsetMinutes, shard: shard, key: key,
                        byteLength: byteLength, objectCRC32: objectCRC32, observed: observed))
                case .dry:
                    dry.append(WeatherFrameShard(offsetMinutes: frame.offsetMinutes, shard: shard))
                case .outOfDomain:
                    break  // `shards(for:)` only ever yields shards of this lattice
                }
            }
        }
        return WeatherPlan(outcome: .covered, fetch: fetch, dry: dry)
    }
}

// MARK: - Shard identity

/// One shard's identity: its column and row on the fixed global shard grid.
///
/// Ordered by **`(row, col)`** — the order the manifest states for `shards[]`, the order
/// ``WeatherLattice/shards(for:)`` returns, and the order the presence bit index
/// `row * shardColumns + col` counts in. Written out rather than synthesised because Swift's
/// memberwise `Comparable` would order by declaration order, and one ordering silently disagreeing
/// with the document is exactly how a binary search over the shard list starts answering `dry` for
/// shards that exist.
public struct WeatherShardID: Hashable, Comparable, Sendable {
    public var column: UInt32
    public var row: UInt32

    public init(column: UInt32, row: UInt32) {
        self.column = column
        self.row = row
    }

    public static func < (lhs: WeatherShardID, rhs: WeatherShardID) -> Bool {
        (lhs.row, lhs.column) < (rhs.row, rhs.column)
    }
}

/// What the manifest says is at one shard of one frame.
public enum WeatherShardState: Equatable, Sendable {
    /// Fetch it; a 404 here is an error, not dry.
    case present(key: String, byteLength: Int, objectCRC32: UInt32, observed: Bool)
    /// Every cell is dry. Nothing to fetch, nothing missing — and intensity 0 to paint.
    case dry
    /// Not a shard of this lattice.
    case outOfDomain
}

/// One published shard of one frame, as the document lists it.
public struct WeatherManifestShard: Equatable, Sendable {
    public var id: WeatherShardID
    public var byteLength: Int
    public var objectCRC32: UInt32
    public var observed: Bool

    public init(id: WeatherShardID, byteLength: Int, objectCRC32: UInt32, observed: Bool) {
        self.id = id
        self.byteLength = byteLength
        self.objectCRC32 = objectCRC32
        self.observed = observed
    }
}

// MARK: - The lattice

/// The lattice, as the manifest states it. Everything a client used to hardcode.
public struct WeatherLattice: Equatable, Sendable {
    public var southLatitudeMicrodegrees: Int32
    public var westLongitudeMicrodegrees: Int32
    /// One cell, **both axes** — the lattice is square in degrees.
    public var cellMicrodegrees: UInt32
    public var width: UInt32
    public var height: UInt32
    public var shardWidth: UInt32
    public var shardHeight: UInt32
    public var shardColumns: UInt32
    public var shardRows: UInt32
    public var tileEdge: UInt16
    public var entriesPerPage: UInt16
    public var cellSizeMetres: UInt16
    /// Lattice rows with a source behind them; outside it every frame is intensity 15 forever.
    public var coveredRows: Range<UInt32>
    /// The prefix of every object key, from the manifest so the tree can move.
    public var keyPrefix: String
    /// The current generation — the key's second segment.
    public var generation: String

    public init(
        southLatitudeMicrodegrees: Int32, westLongitudeMicrodegrees: Int32,
        cellMicrodegrees: UInt32, width: UInt32, height: UInt32,
        shardWidth: UInt32, shardHeight: UInt32, shardColumns: UInt32, shardRows: UInt32,
        tileEdge: UInt16, entriesPerPage: UInt16, cellSizeMetres: UInt16,
        coveredRows: Range<UInt32>, keyPrefix: String, generation: String
    ) {
        self.southLatitudeMicrodegrees = southLatitudeMicrodegrees
        self.westLongitudeMicrodegrees = westLongitudeMicrodegrees
        self.cellMicrodegrees = cellMicrodegrees
        self.width = width
        self.height = height
        self.shardWidth = shardWidth
        self.shardHeight = shardHeight
        self.shardColumns = shardColumns
        self.shardRows = shardRows
        self.tileEdge = tileEdge
        self.entriesPerPage = entriesPerPage
        self.cellSizeMetres = cellSizeMetres
        self.coveredRows = coveredRows
        self.keyPrefix = keyPrefix
        self.generation = generation
    }

    /// How many shards this lattice has.
    ///
    /// Overflow-checked rather than a plain product: a parsed lattice can never overflow here
    /// (``WeatherManifestV2/maximumShards`` is enforced at parse time), but this type is public and
    /// constructible directly, and a trapping multiplication in a reader whose input is a document
    /// off the network is the wrong failure mode to leave lying around. A saturated count answers
    /// "more shards than any document may claim", which every caller already refuses.
    public var shardCount: UInt32 {
        let (product, overflowed) = shardColumns.multipliedReportingOverflow(by: shardRows)
        return overflowed ? UInt32.max : product
    }

    /// The bit index of a shard in a frame's presence bitmap.
    public func bit(of shard: WeatherShardID) -> UInt32? {
        guard shard.column < shardColumns, shard.row < shardRows else { return nil }
        // Widened for the same reason `shardCount` is: `row * shardColumns` is bounded by the
        // shard count, which is bounded only for a *parsed* lattice.
        let index = UInt64(shard.row) * UInt64(shardColumns) + UInt64(shard.column)
        guard index <= UInt64(UInt32.max) else { return nil }
        return UInt32(index)
    }

    /// The object key of one shard of one frame. Composed, never read from the document.
    public func shardKey(offsetMinutes: UInt32, shard: WeatherShardID) -> String {
        "\(keyPrefix)/\(generation)/f\(offsetMinutes)/s\(shard.column)-\(shard.row).obcg"
    }

    /// The geographic window of one shard, half-open `[south, north) x [west, east)`. `nil` for a
    /// shard off the lattice, for the same reason ``geometry(of:)`` is optional.
    public func bounds(of shard: WeatherShardID) -> WeatherBoundingBox? {
        geometry(of: shard)?.bounds
    }

    /// The OBCG geometry a shard object must declare — the manifest's arithmetic, so a corridor's
    /// page and tile ranges are plannable before a byte is fetched and checkable against the header
    /// once it arrives.
    ///
    /// **Edge shards are short.** The lattice need not be a whole number of shards wide or high, so
    /// the last column and the last row carry `width = lattice.width - col * shardWidth` cells.
    /// Assuming a full square there is how a client reads a neighbouring shard's bytes as this
    /// shard's north edge.
    ///
    /// `nil` for a shard that is not on this lattice — the twin of `Grid::shard_geometry`'s opening
    /// `self.bit_of(shard)?`. Without that guard `width - columnOrigin` underflows for an off-grid
    /// column and the arithmetic traps, which is reachable through the public ``bounds(of:)``.
    public func geometry(of shard: WeatherShardID) -> WeatherFrameGeometry? {
        guard bit(of: shard) != nil else { return nil }
        let columnOrigin = UInt64(shard.column) * UInt64(shardWidth)
        let rowOrigin = UInt64(shard.row) * UInt64(shardHeight)
        let shardCells = UInt64(cellMicrodegrees)
        return WeatherFrameGeometry(
            southMicrodegrees: Int32(Int64(southLatitudeMicrodegrees) + Int64(rowOrigin * shardCells)),
            westMicrodegrees: Int32(Int64(westLongitudeMicrodegrees) + Int64(columnOrigin * shardCells)),
            cellLatitudeMicrodegrees: cellMicrodegrees,
            cellLongitudeMicrodegrees: cellMicrodegrees,
            width: UInt32(min(UInt64(shardWidth), UInt64(width) - columnOrigin)),
            height: UInt32(min(UInt64(shardHeight), UInt64(height) - rowOrigin)),
            cellSizeMetres: cellSizeMetres, tileEdge: tileEdge, entriesPerPage: entriesPerPage)
    }

    /// Every shard covering `bbox`, ascending by `(row, col)` — **the whole of what used to be
    /// product selection**, and it is a handful of divisions.
    ///
    /// Coordinates are microdegrees in the **-180..180 / -90..90** convention, and that is checked
    /// rather than assumed: a longitude in the 0..360 form (352,150,000 meaning -7.85 degrees) is
    /// ``WeatherBboxError/outOfRange``, never silently reinterpreted, because the alternative is a
    /// corridor answered from the wrong hemisphere with no error anywhere. `west > east` is not
    /// malformed — it **means the window crosses the antimeridian**, and it is served by splitting
    /// into `[west, 180)` and `[-180, east)`. See `OBCG_Spec.md` §10.2, which is normative for all of
    /// this, and note that an empty result is *not* "everywhere dry":
    /// ``WeatherManifestV2/plan(bbox:now:)`` reports it as ``WeatherPlanOutcome/outOfDomain``.
    public func shards(for bbox: WeatherBoundingBox) throws -> [WeatherShardID] {
        try bbox.validateAsWindow()
        guard let (firstRow, lastRow) = cellSpan(
            low: bbox.southMicrodegrees, high: bbox.northMicrodegrees,
            origin: Int64(southLatitudeMicrodegrees), extent: height)
        else { return [] }
        // One interval normally; two when the window crosses the antimeridian.
        let spans: [(Int64, Int64)] = bbox.westMicrodegrees < bbox.eastMicrodegrees
            ? [(bbox.westMicrodegrees, bbox.eastMicrodegrees)]
            : [
                (bbox.westMicrodegrees, Int64(180_000_000)),
                (Int64(-180_000_000), bbox.eastMicrodegrees),
            ]
        var columns: Set<UInt32> = []
        for (low, high) in spans where low < high {
            guard let (firstColumn, lastColumn) = cellSpan(
                low: low, high: high, origin: Int64(westLongitudeMicrodegrees), extent: width)
            else { continue }
            for column in (firstColumn / shardWidth)...(lastColumn / shardWidth) {
                columns.insert(column)
            }
        }
        let ordered = columns.sorted()
        var shards: [WeatherShardID] = []
        for row in (firstRow / shardHeight)...(lastRow / shardHeight) {
            shards.append(contentsOf: ordered.map { WeatherShardID(column: $0, row: row) })
        }
        return shards
    }

    /// Do any of the lattice rows this bbox touches have a **source** behind them?
    ///
    /// `coveredRows` is not decoration: rows outside it are published as intensity 15 in every frame,
    /// forever, because no source we ingest reaches them (#1242's polar band). A corridor wholly
    /// inside that band has objects it *could* fetch, and they would all decode to "we do not know" —
    /// so ``WeatherManifestV2/plan(bbox:now:)`` answers ``WeatherPlanOutcome/uncovered`` instead of
    /// issuing nine Range reads to learn a permanent fact the manifest already stated.
    ///
    /// **Internal on purpose.** It answers with a bare `Bool` and does not validate its bbox, so a
    /// caller handing it a 0..360 longitude would get a confident wrong answer with nowhere to report
    /// the problem. Its one call site reaches it only after ``shards(for:)`` has accepted the window.
    func anyRowHasASource(in bbox: WeatherBoundingBox) -> Bool {
        guard let (firstRow, lastRow) = cellSpan(
            low: bbox.southMicrodegrees, high: bbox.northMicrodegrees,
            origin: Int64(southLatitudeMicrodegrees), extent: height)
        else { return false }
        return firstRow < coveredRows.upperBound && lastRow >= coveredRows.lowerBound
    }

    /// The lattice-cell window covering `bbox`, clamped to the lattice — or `nil` when the two do not
    /// overlap. The frame the corridor is assembled into is exactly this window, so it is derived
    /// from the same ``cellSpan`` the shard set is: a window one cell wider than the shards planned
    /// for would leave a stripe of no-data down the east edge of every frame.
    ///
    /// Antimeridian-wrapping windows are refused here rather than split. ``shards(for:)`` splits
    /// because a *shard set* can straddle the date line; an OBCW window cannot (`OBCW_Spec.md` §1),
    /// and the corridor is clamped at the date line for exactly that reason.
    func cellWindow(for bbox: WeatherBoundingBox) -> WeatherCellWindow? {
        guard bbox.westMicrodegrees < bbox.eastMicrodegrees else { return nil }
        guard let (firstRow, lastRow) = cellSpan(
                  low: bbox.southMicrodegrees, high: bbox.northMicrodegrees,
                  origin: Int64(southLatitudeMicrodegrees), extent: height),
              let (firstColumn, lastColumn) = cellSpan(
                  low: bbox.westMicrodegrees, high: bbox.eastMicrodegrees,
                  origin: Int64(westLongitudeMicrodegrees), extent: width)
        else { return nil }
        let cell = Int64(cellMicrodegrees)
        let clipped = bbox.southMicrodegrees < Int64(southLatitudeMicrodegrees)
            || bbox.westMicrodegrees < Int64(westLongitudeMicrodegrees)
            || bbox.northMicrodegrees > Int64(southLatitudeMicrodegrees) + Int64(height) * cell
            || bbox.eastMicrodegrees > Int64(westLongitudeMicrodegrees) + Int64(width) * cell
        return WeatherCellWindow(
            columnMinimum: Int(firstColumn), rowMinimum: Int(firstRow),
            width: Int(lastColumn - firstColumn) + 1, height: Int(lastRow - firstRow) + 1,
            isClipped: clipped)
    }

    /// The half-open cell interval `[low, high)` of one axis, intersected with the lattice — or `nil`
    /// if the two do not overlap.
    ///
    /// **The intersection is tested on the unclamped interval.** Clamping first is the bug this
    /// spells out: an interval lying wholly east of the lattice collapses onto its last column
    /// instead of vanishing, and a rider off the map is served another continent's shard rather than
    /// told there is nothing there. An edge landing exactly on a cell boundary closes the cell before
    /// it, which `ceil - 1` gives and a plain floor does not.
    private func cellSpan(
        low: Int64, high: Int64, origin: Int64, extent: UInt32
    ) -> (UInt32, UInt32)? {
        let cell = Int64(cellMicrodegrees)
        let first = Swift.max(0, floorDivide(low - origin, cell))
        let last = Swift.min(Int64(extent) - 1, floorDivide(high - origin + cell - 1, cell) - 1)
        guard first <= last else { return nil }
        return (UInt32(first), UInt32(last))
    }
}

/// A rectangle of lattice cells, row 0 southernmost, column 0 westernmost.
struct WeatherCellWindow: Equatable {
    var columnMinimum: Int
    var rowMinimum: Int
    var width: Int
    var height: Int
    /// True when the corridor reaches outside the lattice, so the frame answers only part of the
    /// question. It becomes OBCW's partial-coverage flag — never a silent smaller map.
    var isClipped: Bool
}

/// Floor division for negative numerators — Swift's `/` truncates toward zero, which would fold a
/// coordinate west of the origin onto cell 0 instead of off the lattice.
func floorDivide(_ numerator: Int64, _ denominator: Int64) -> Int64 {
    precondition(denominator > 0)
    let quotient = numerator / denominator
    return numerator % denominator < 0 ? quotient - 1 : quotient
}

/// Ceiling division, the counterpart used when a window edge must round outward to a whole cell.
func ceilDivide(_ numerator: Int64, _ denominator: Int64) -> Int64 {
    precondition(denominator > 0)
    return -floorDivide(-numerator, denominator)
}

/// Ceiling division over unsigned wire values, **without the `(value + divisor - 1)` step**.
///
/// That step is the classic overflow: with `width` straight off the network and near `UInt32.max`
/// the addition traps. Rust reaches for `div_ceil`, which cannot overflow; this is the same
/// arithmetic spelled out.
func divideRoundingUp(_ value: UInt32, _ divisor: UInt32) -> UInt32 {
    precondition(divisor > 0)
    return value / divisor + (value % divisor == 0 ? 0 : 1)
}

// MARK: - Cadence and freshness

public struct WeatherCadence: Equatable, Sendable {
    public var frameStepMinutes: UInt32
    public var frames: UInt32
    /// How far from its stated validity a cell's underlying source frame may have been.
    public var maximumSourceSkew: TimeInterval

    public init(frameStepMinutes: UInt32, frames: UInt32, maximumSourceSkew: TimeInterval) {
        self.frameStepMinutes = frameStepMinutes
        self.frames = frames
        self.maximumSourceSkew = maximumSourceSkew
    }
}

/// Deadlines, absolute, from the document. A client compares timestamps; it holds no constants.
public struct WeatherFreshness: Equatable, Sendable {
    public var manifestMaximumAge: TimeInterval
    public var nextGenerationExpectedAt: Date
    public var staleAfter: Date

    public init(
        manifestMaximumAge: TimeInterval, nextGenerationExpectedAt: Date, staleAfter: Date
    ) {
        self.manifestMaximumAge = manifestMaximumAge
        self.nextGenerationExpectedAt = nextGenerationExpectedAt
        self.staleAfter = staleAfter
    }

    /// Inclusive: the generation is usable up to and including its deadline second. Past it there is
    /// **no weather** — which is not the same as no rain, and must never render as dry.
    public func isUsable(at now: Date) -> Bool { now <= staleAfter }

    /// Should this document be re-fetched before being used again?
    public func manifestIsStale(fetchedAt: Date, now: Date) -> Bool {
        now.timeIntervalSince(fetchedAt) > manifestMaximumAge
    }
}

// MARK: - Frames

/// One frame of the timeline.
///
/// `presence` and `shards` are **private**, and that is load-bearing rather than tidy. They are two
/// spellings of one fact, proved equal exactly once at parse time; leaving them settable would let a
/// caller desync them and get a silent `dry` for a shard the manifest says exists — the forbidden
/// answer, reached through the one defaulting branch in the module. With them private there is no
/// defaulting branch at all: ``state(of:in:)`` looks the shard up in the sorted list, and *that
/// lookup is* the bitmap read.
public struct WeatherManifestFrame: Equatable, Sendable {
    public var offsetMinutes: UInt32
    public var validAt: Date
    /// Presence, one bit per shard, `row * shardColumns + col`.
    private var presence: [UInt8]
    /// Exactly the shards `presence` names, ascending by `(row, col)`.
    private var shardEntries: [WeatherManifestShard]

    init(
        offsetMinutes: UInt32, validAt: Date, presence: [UInt8],
        shardEntries: [WeatherManifestShard]
    ) {
        self.offsetMinutes = offsetMinutes
        self.validAt = validAt
        self.presence = presence
        self.shardEntries = shardEntries
    }

    /// Every published shard of this frame, ascending by `(row, col)`.
    public var shards: [WeatherManifestShard] { shardEntries }

    /// The presence bit as the document spells it. Equal to `state(of:) == .present` by
    /// construction; kept because a probe or a diagnostics panel wants the bitmap's own answer, and
    /// the equivalence test pins that the two cannot diverge.
    public func isPresent(_ shard: WeatherShardID, in lattice: WeatherLattice) -> Bool {
        guard let bit = lattice.bit(of: shard) else { return false }
        let byte = Int(bit / 8)
        guard byte < presence.count else { return false }
        return presence[byte] & (1 << (bit % 8)) != 0
    }

    /// The three-valued answer. This is the function "a 404 must not mean dry" reduces to.
    public func state(of shard: WeatherShardID, in lattice: WeatherLattice) -> WeatherShardState {
        guard lattice.bit(of: shard) != nil else { return .outOfDomain }
        var low = 0
        var high = shardEntries.count
        while low < high {
            let middle = (low + high) / 2
            if shardEntries[middle].id < shard { low = middle + 1 } else { high = middle }
        }
        guard low < shardEntries.count, shardEntries[low].id == shard else { return .dry }
        let entry = shardEntries[low]
        return .present(
            key: lattice.shardKey(offsetMinutes: offsetMinutes, shard: shard),
            byteLength: entry.byteLength, objectCRC32: entry.objectCRC32, observed: entry.observed)
    }
}

// MARK: - The plan

/// Why a plan has no objects in it — or that it does.
///
/// **Every one of these is a different thing to show a rider, and only one of them is rain.** An
/// empty array cannot say which, so it is not what ``WeatherManifestV2/plan(bbox:now:)`` returns: the
/// fetch path must switch on this, and the compiler will not let it render "off the map" or "no
/// weather" as "no rain".
public enum WeatherPlanOutcome: Equatable, Sendable {
    /// The dataset answers this bbox. `fetch` and `dry` describe it — and `fetch` being empty with
    /// `dry` populated is the *real* "no rain anywhere near you".
    case covered
    /// The bbox is off the lattice, or is not a window this client will interpret. There is no answer
    /// here — which is not an answer of "no rain".
    case outOfDomain
    /// On the lattice, but every row it touches is outside `coveredRows`: no source reaches it in any
    /// frame, ever. The objects exist and are entirely intensity 15, so fetching them would buy nine
    /// round trips and the word "unknown".
    case uncovered
    /// This generation is past its `stale_after` and no fresher manifest replaced it. **No weather** —
    /// the rider is owed that sentence, and never a dry map.
    case expired
}

/// One object to fetch, with everything needed to verify it.
public struct WeatherPlannedRead: Equatable, Sendable {
    public var offsetMinutes: UInt32
    public var shard: WeatherShardID
    public var key: String
    public var byteLength: Int
    public var objectCRC32: UInt32
    public var observed: Bool

    public init(
        offsetMinutes: UInt32, shard: WeatherShardID, key: String, byteLength: Int,
        objectCRC32: UInt32, observed: Bool
    ) {
        self.offsetMinutes = offsetMinutes
        self.shard = shard
        self.key = key
        self.byteLength = byteLength
        self.objectCRC32 = objectCRC32
        self.observed = observed
    }
}

/// One `(frame, shard)` pair the baker measured as dry everywhere.
public struct WeatherFrameShard: Hashable, Sendable {
    public var offsetMinutes: UInt32
    public var shard: WeatherShardID

    public init(offsetMinutes: UInt32, shard: WeatherShardID) {
        self.offsetMinutes = offsetMinutes
        self.shard = shard
    }
}

/// The outcomes of a corridor, kept apart by construction.
public struct WeatherPlan: Equatable, Sendable {
    public var outcome: WeatherPlanOutcome
    /// Objects that MUST exist. Empty outside ``WeatherPlanOutcome/covered``.
    public var fetch: [WeatherPlannedRead]
    /// Shards the baker measured as dry everywhere. Empty outside `covered`.
    public var dry: [WeatherFrameShard]

    public init(outcome: WeatherPlanOutcome, fetch: [WeatherPlannedRead], dry: [WeatherFrameShard]) {
        self.outcome = outcome
        self.fetch = fetch
        self.dry = dry
    }
}

// MARK: - Errors

/// Why a bbox is not a window this client will answer.
///
/// Both are caller bugs rather than weather, and both are reported rather than repaired: a client
/// that clamps a malformed corridor answers the wrong question confidently, which is worse than
/// answering none.
public enum WeatherBboxError: Error, Equatable, Sendable {
    /// A coordinate outside ±90° latitude or ±180° longitude. Longitudes are **-180..180**; the
    /// 0..360 spelling is this error.
    case outOfRange
    /// A window with no area: `south >= north`, or `west == east`.
    case empty
}

/// Why a manifest document could not be used at all. Every case degrades to a service outage — the
/// hourly forecast still works, and the rider is told there is no rain map.
public enum WeatherManifestError: Error, Equatable, Sendable {
    case malformed
    case unsupportedVersion(Int)
}

// MARK: - Frame geometry

/// One shard's exact OBCG geometry, derived from the stated lattice so corridor page arithmetic is
/// plannable before a single byte is fetched — and verifiable against the header once it is.
public struct WeatherFrameGeometry: Equatable, Sendable {
    public var southMicrodegrees: Int32
    public var westMicrodegrees: Int32
    public var cellLatitudeMicrodegrees: UInt32
    public var cellLongitudeMicrodegrees: UInt32
    public var width: UInt32
    public var height: UInt32
    public var cellSizeMetres: UInt16
    public var tileEdge: UInt16
    public var entriesPerPage: UInt16

    public init(
        southMicrodegrees: Int32, westMicrodegrees: Int32, cellLatitudeMicrodegrees: UInt32,
        cellLongitudeMicrodegrees: UInt32, width: UInt32, height: UInt32, cellSizeMetres: UInt16,
        tileEdge: UInt16, entriesPerPage: UInt16
    ) {
        self.southMicrodegrees = southMicrodegrees
        self.westMicrodegrees = westMicrodegrees
        self.cellLatitudeMicrodegrees = cellLatitudeMicrodegrees
        self.cellLongitudeMicrodegrees = cellLongitudeMicrodegrees
        self.width = width
        self.height = height
        self.cellSizeMetres = cellSizeMetres
        self.tileEdge = tileEdge
        self.entriesPerPage = entriesPerPage
    }

    public var bounds: WeatherBoundingBox {
        WeatherBoundingBox(
            southMicrodegrees: Int64(southMicrodegrees),
            westMicrodegrees: Int64(westMicrodegrees),
            northMicrodegrees: Int64(southMicrodegrees)
                + Int64(height) * Int64(cellLatitudeMicrodegrees),
            eastMicrodegrees: Int64(westMicrodegrees)
                + Int64(width) * Int64(cellLongitudeMicrodegrees))
    }

    /// Everything the OBCG header must agree with. A shard whose fetched header contradicts the
    /// lattice the manifest stated is refused: one of the two is lying, and neither is worth guessing
    /// about.
    func agrees(with header: OBCGridHeader) -> Bool {
        header.southLatitudeMicrodegrees == southMicrodegrees
            && header.westLongitudeMicrodegrees == westMicrodegrees
            && header.cellLatitudeStrideMicrodegrees == cellLatitudeMicrodegrees
            && header.cellLongitudeStrideMicrodegrees == cellLongitudeMicrodegrees
            && header.width == width && header.height == height
            && header.cellSizeMetres == cellSizeMetres && header.tileEdge == tileEdge
            && header.entriesPerPage == entriesPerPage
    }
}

// MARK: - Parsing

public extension WeatherManifestV2 {
    /// Parse and validate a manifest document.
    ///
    /// Two different strictnesses, deliberately:
    ///
    /// - the **document** is strict — bad JSON, an unknown `version`, a lattice this client cannot
    ///   address, a key prefix that would steer the client off its own tree, or a retention chain
    ///   longer than OBCG §10.4 allows is an outage, because a client that guesses at any of them
    ///   will eventually guess wrong about what exists;
    /// - a **frame** is lenient — a frame this build cannot make sense of is skipped and counted,
    ///   never fatal. One malformed frame must not cost a rider the whole timeline.
    static func parse(_ data: Data) throws -> WeatherManifestV2 {
        // **`2.0` is not `2`.** `JSONDecoder` folds an integral `Double` into `Int`, so a document
        // saying `"version": 2.0` would decode as version 2 and be answered as a format it is not
        // written to; serde refuses it outright. The distinction survives nowhere in the decoded
        // model — only in the raw JSON number — so it is read there, and `CFNumberIsFloatType` is
        // the only way to ask `JSONSerialization` what the wire actually spelled.
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw WeatherManifestError.malformed
        }
        if let version = root["version"] as? NSNumber, CFNumberIsFloatType(version) {
            throw WeatherManifestError.malformed
        }
        let document: Document
        do {
            document = try JSONDecoder().decode(Document.self, from: data)
        } catch {
            throw WeatherManifestError.malformed
        }
        guard document.version == supportedVersion else {
            throw WeatherManifestError.unsupportedVersion(document.version)
        }
        guard let generatedAt = RFC3339.parse(document.generated_at),
              let referenceTime = RFC3339.parse(document.reference_time),
              // The generation is a key segment and the key prefix is joined onto the service
              // origin: a manifest must not be able to steer the client off its own tree.
              isSafeKeySegment(document.generation), isSafeKeyPrefix(document.key_prefix),
              // §10.4 caps the chain at two, normatively: a longer list means this client and the
              // service's sweep disagree about which generations exist, and guessing which of them
              // is right is how a client ends up reading an object a sweep already collected.
              document.previous_generations.count <= retainedPreviousGenerations,
              document.previous_generations.allSatisfy(isSafeKeySegment),
              let nextGeneration = RFC3339.parse(document.freshness.next_generation_expected_at),
              let staleAfter = RFC3339.parse(document.freshness.stale_after),
              // A generation that expires before its own replacement is due is a mis-derived cycle.
              staleAfter >= nextGeneration
        else { throw WeatherManifestError.malformed }

        let lattice = try document.lattice.validated(
            keyPrefix: document.key_prefix, generation: document.generation)

        var frames: [WeatherManifestFrame] = []
        var skipped = 0
        for entry in document.frames {
            if let frame = entry.frame?.validated(lattice: lattice) {
                frames.append(frame)
            } else {
                skipped += 1
            }
        }
        // §10: frames are a timeline. Out-of-order or duplicated validities make the OBCW re-encode
        // (which requires strictly increasing `valid_at`) unbuildable later, so refuse now. And
        // `offset_min` alone names the object, so two frames sharing one would name the same object
        // at two validities, with `frame(offsetMinutes:)` silently answering with the first.
        for index in 1..<Swift.max(1, frames.count) {
            guard frames[index].validAt > frames[index - 1].validAt,
                  frames[index].offsetMinutes != frames[index - 1].offsetMinutes
            else { throw WeatherManifestError.malformed }
        }
        // Cheap, derivable, and catching a mis-derived cycle: the frame count the cadence promises
        // is counted before leniency removes anything, so a skipped frame cannot mask a short list.
        guard document.frames.count == Int(document.cadence.frames) else {
            throw WeatherManifestError.malformed
        }

        return WeatherManifestV2(
            generation: document.generation, generatedAt: generatedAt,
            referenceTime: referenceTime, previousGenerations: document.previous_generations,
            lattice: lattice,
            cadence: WeatherCadence(
                frameStepMinutes: document.cadence.frame_step_min,
                frames: document.cadence.frames,
                maximumSourceSkew: TimeInterval(document.cadence.max_source_skew_s)),
            freshness: WeatherFreshness(
                manifestMaximumAge: TimeInterval(Swift.max(0, document.freshness.manifest_max_age_s)),
                nextGenerationExpectedAt: nextGeneration, staleAfter: staleAfter),
            attributions: document.attribution.map {
                WeatherAttribution(text: $0.text, url: $0.url, sourceID: $0.source_id)
            },
            frames: frames, skippedFrames: skipped)
    }

    // MARK: Wire shape

    private struct Document: Decodable {
        var version: Int
        var generation: String
        var generated_at: String
        var reference_time: String
        var key_prefix: String
        var previous_generations: [String]
        var lattice: LatticeEntry
        var cadence: CadenceEntry
        var freshness: FreshnessEntry
        var attribution: [AttributionEntry]
        var frames: [LenientFrameEntry]

        /// Written out because Swift's synthesised `Decodable` does not apply a property's default
        /// value to a missing key. The two optional arrays are optional in the schema — a first
        /// generation has no predecessors, and a dataset may credit nobody — and requiring them
        /// would refuse documents `host/obc-wx-bake` is allowed to write.
        init(from decoder: any Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            version = try container.decode(Int.self, forKey: .version)
            generation = try container.decode(String.self, forKey: .generation)
            generated_at = try container.decode(String.self, forKey: .generated_at)
            reference_time = try container.decode(String.self, forKey: .reference_time)
            key_prefix = try container.decode(String.self, forKey: .key_prefix)
            previous_generations = try Self.optionalList(
                [String].self, container, .previous_generations)
            lattice = try container.decode(LatticeEntry.self, forKey: .lattice)
            cadence = try container.decode(CadenceEntry.self, forKey: .cadence)
            freshness = try container.decode(FreshnessEntry.self, forKey: .freshness)
            attribution = try Self.optionalList([AttributionEntry].self, container, .attribution)
            frames = try container.decode([LenientFrameEntry].self, forKey: .frames)
        }

        /// A list the schema lets a document omit — **omit**, not null.
        ///
        /// `decodeIfPresent` conflates the two, and they are not the same statement: a missing
        /// `previous_generations` is the first generation ever published, while an explicit
        /// `null` is a document asserting something that is not a list. serde refuses the null and
        /// defaults only the absence, so this reader does too, or the two clients keep different
        /// documents (`rejection_equivalence` pins both halves).
        private static func optionalList<T: Decodable>(
            _ type: T.Type, _ container: KeyedDecodingContainer<CodingKeys>, _ key: CodingKeys
        ) throws -> T where T: ExpressibleByArrayLiteral {
            guard container.contains(key) else { return [] }
            guard try !container.decodeNil(forKey: key) else {
                throw WeatherManifestError.malformed
            }
            return try container.decode(T.self, forKey: key)
        }

        enum CodingKeys: String, CodingKey {
            case version, generation, generated_at, reference_time, key_prefix
            case previous_generations, lattice, cadence, freshness, attribution, frames
        }
    }

    /// One element of `frames[]`, decoded so that a *broken element* cannot fail the array.
    ///
    /// This is what makes "frame-lenient" real rather than merely semantic. Decoding `[FrameEntry]`
    /// directly means one frame with a malformed bitmap throws out of the array decode and takes the
    /// whole timeline with it. Catching inside the element keeps the failure the size of the thing
    /// that failed; the *document* stays strict, and the skip is counted.
    private struct LenientFrameEntry: Decodable {
        var frame: FrameEntry?

        init(from decoder: any Decoder) throws {
            frame = try? FrameEntry(from: decoder)
        }
    }

    private struct LatticeEntry: Decodable {
        var south_lat_udeg: Int32
        var west_lon_udeg: Int32
        var cell_udeg: UInt32
        var width: UInt32
        var height: UInt32
        var shard_width: UInt32
        var shard_height: UInt32
        var shard_cols: UInt32
        var shard_rows: UInt32
        var tile_edge: UInt16
        var entries_per_page: UInt16
        var cell_size_m: UInt16
        var covered_rows: RowRangeEntry

        /// The lattice is **document-level**: a client that cannot address the dataset has nothing to
        /// degrade to, so an unusable one is a hard failure rather than a skipped entry.
        func validated(keyPrefix: String, generation: String) throws -> WeatherLattice {
            guard cell_udeg > 0, width > 0, height > 0, shard_width > 0, shard_height > 0
            else { throw WeatherManifestError.malformed }
            // **Bound the lattice, not just the shard, and bound it before dividing by it.** Every
            // number below arrives straight off the wire as a `UInt32`, and a document is free to
            // say `width: 4294967295`: `(width + shard_width - 1)` then overflows and Swift *traps*,
            // which is a crash reachable from the one document every rider fetches first. Two
            // bounds, both geometric rather than arbitrary, both mirroring `validate_grid`:
            //
            // - an axis cannot hold more cells than 360 (or 180) degrees do at the lattice's own
            //   cell pitch, so anything past that is not a description of this planet;
            // - a shard grid is capped at ``WeatherManifestV2/maximumShards``, which is three orders
            //   of magnitude above the production layout and still keeps the presence bitmap a few
            //   kilobytes.
            func axisLimit(_ degrees: UInt32) -> UInt32 { degrees * 1_000_000 / cell_udeg + 1 }
            guard width <= axisLimit(360), height <= axisLimit(180) else {
                throw WeatherManifestError.malformed
            }
            let (shardTotal, shardOverflow) = shard_cols.multipliedReportingOverflow(by: shard_rows)
            guard !shardOverflow, shardTotal <= WeatherManifestV2.maximumShards else {
                throw WeatherManifestError.malformed
            }
            guard // The shard grid must be exactly the one that tiles the lattice, or the client's
                  // arithmetic and the baker's disagree about which object holds a cell. Written as
                  // a remainder test rather than `(width + shard_width - 1) / shard_width` so it
                  // cannot overflow whatever the axis bound above lets through.
                  shard_cols == divideRoundingUp(width, shard_width),
                  shard_rows == divideRoundingUp(height, shard_height),
                  // OBCG §1/§3, checked before a byte is fetched: a shard the header could only
                  // reject is not worth a Range read.
                  UInt64(shard_width) * UInt64(shard_height) <= OBCGridCodec.maximumGridCells,
                  shard_width <= OBCGridCodec.maximumGridDimension,
                  shard_height <= OBCGridCodec.maximumGridDimension,
                  tile_edge >= OBCGridCodec.minimumTileEdge,
                  tile_edge <= OBCGridCodec.maximumTileEdge, tile_edge.nonzeroBitCount == 1,
                  entries_per_page > 0, entries_per_page <= OBCGridCodec.maximumEntriesPerPage,
                  cell_size_m > 0,
                  covered_rows.start <= covered_rows.end, covered_rows.end <= height
            else { throw WeatherManifestError.malformed }
            return WeatherLattice(
                southLatitudeMicrodegrees: south_lat_udeg,
                westLongitudeMicrodegrees: west_lon_udeg, cellMicrodegrees: cell_udeg,
                width: width, height: height, shardWidth: shard_width, shardHeight: shard_height,
                shardColumns: shard_cols, shardRows: shard_rows, tileEdge: tile_edge,
                entriesPerPage: entries_per_page, cellSizeMetres: cell_size_m,
                coveredRows: covered_rows.start..<covered_rows.end,
                keyPrefix: keyPrefix, generation: generation)
        }
    }

    private struct RowRangeEntry: Decodable {
        var start: UInt32
        var end: UInt32
    }

    private struct CadenceEntry: Decodable {
        var frame_step_min: UInt32
        var frames: UInt32
        var max_source_skew_s: Int64
    }

    private struct FreshnessEntry: Decodable {
        var manifest_max_age_s: Int64
        var next_generation_expected_at: String
        var stale_after: String
    }

    private struct AttributionEntry: Decodable {
        var source_id: String
        var text: String
        var url: String
    }

    private struct FrameEntry: Decodable {
        var offset_min: UInt32
        var valid_at: String
        var present: String
        var shards: [ShardEntry]

        func validated(lattice: WeatherLattice) -> WeatherManifestFrame? {
            guard let validAt = RFC3339.parse(valid_at), let presence = unhex(present) else {
                return nil
            }
            // `UInt64` throughout: `shardCount` saturates at `UInt32.max` for a lattice nobody
            // validated, and `(count + 7)` would then trap on the way to the byte count.
            let count = UInt64(lattice.shardCount)
            guard UInt64(presence.count) == (count + 7) / 8 else { return nil }
            // Bits past the last shard must be zero, or "how many shards are there" has two answers.
            for bit in count..<(UInt64(presence.count) * 8)
            where presence[Int(bit / 8)] & (UInt8(1) << UInt8(bit % 8)) != 0 { return nil }

            var entries: [WeatherManifestShard] = []
            entries.reserveCapacity(shards.count)
            for shard in shards {
                let id = WeatherShardID(column: shard.col, row: shard.row)
                guard let bit = lattice.bit(of: id),
                      // The bitmap and the list are one statement, so a frame where they disagree is
                      // refused rather than reconciled: silently trusting either one is how a dry
                      // shard becomes a missing object, or the reverse.
                      presence[Int(bit / 8)] & (1 << (bit % 8)) != 0,
                      let crc = parseCRC32(shard.object_crc32),
                      shard.bytes > 0, shard.bytes <= UInt64(Int32.max)
                else { return nil }
                entries.append(WeatherManifestShard(
                    id: id, byteLength: Int(shard.bytes), objectCRC32: crc,
                    observed: shard.observed))
            }
            // **Stable, and stable on purpose.** `Array.sort` is not, so with two entries for one
            // shard which of them survived the dedup below was luck — while Rust's `sort_by_key` is
            // stable and keeps the first in document order. Two clients disagreeing about which
            // `bytes`/`object_crc32` a duplicated shard has is one of them refusing an object the
            // other fetches, so the tie is broken by the document's own order.
            let ordered = entries.enumerated()
                .sorted { $0.element.id == $1.element.id ? $0.offset < $1.offset
                                                         : $0.element.id < $1.element.id }
                .map(\.element)
            var deduplicated: [WeatherManifestShard] = []
            for entry in ordered where deduplicated.last?.id != entry.id {
                deduplicated.append(entry)
            }
            let flagged = presence.reduce(0) { $0 + $1.nonzeroBitCount }
            guard deduplicated.count == flagged else { return nil }
            return WeatherManifestFrame(
                offsetMinutes: offset_min, validAt: validAt, presence: presence,
                shardEntries: deduplicated)
        }
    }

    private struct ShardEntry: Decodable {
        var col: UInt32
        var row: UInt32
        var bytes: UInt64
        var object_crc32: String
        var observed: Bool
    }
}

/// A key segment the client is willing to put in a URL: no separators, no traversal, no emptiness.
private func isSafeKeySegment(_ text: String) -> Bool {
    !text.isEmpty && text.count <= 64
        && text.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_") }
}

private func isSafeKeyPrefix(_ text: String) -> Bool {
    !text.isEmpty && !text.hasPrefix("/") && !text.hasSuffix("/") && !text.contains("..")
        && text.split(separator: "/", omittingEmptySubsequences: false)
            .allSatisfy { isSafeKeySegment(String($0)) }
}

/// A `0x`-prefixed 32-bit hex integer, and nothing else.
///
/// Swift's `UInt32(_:radix:)` accepts a leading `+`, exactly as Rust's `from_str_radix` does, so
/// `"0x+1A00000"` used to parse in both clients — the review found them agreeing on an answer
/// neither had decided. A CRC is at most eight hex digits; a sign is not one of them. The `0x` is
/// lowercase because that is what the document writes, and `0X` is a different string this reader
/// does not normalise.
private func parseCRC32(_ text: String) -> UInt32? {
    guard text.hasPrefix("0x") else { return nil }
    let digits = text.dropFirst(2)
    guard !digits.isEmpty, digits.count <= 8,
          digits.allSatisfy({ $0.isHexDigit && $0.isASCII })
    else { return nil }
    return UInt32(digits, radix: 16)
}

private func unhex(_ text: String) -> [UInt8]? {
    let characters = Array(text)
    guard !characters.isEmpty, characters.count % 2 == 0 else { return nil }
    var bytes: [UInt8] = []
    bytes.reserveCapacity(characters.count / 2)
    for index in stride(from: 0, to: characters.count, by: 2) {
        guard let byte = UInt8(String(characters[index...index + 1]), radix: 16) else { return nil }
        bytes.append(byte)
    }
    return bytes
}
