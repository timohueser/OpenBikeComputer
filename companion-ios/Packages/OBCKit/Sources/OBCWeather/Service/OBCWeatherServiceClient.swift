import Foundation
import OBCWeatherWire

/// The client for the OBC weather service: one mutable manifest and a lot of immutable shard objects
/// in object storage, and nothing else.
///
/// What the service learns about a rider is the point of the design. There is no per-request compute,
/// no account and no coordinate in any URL — the only locality signal is *which tile indexes* a Range
/// header asks for inside an immutable object, and even that is a 90 km disc rather than a position.
/// MET is the sole third party that receives an actual coordinate (WX1), and WX13 documents both
/// facts to the rider.
///
/// The whole path is: manifest v2 → validate and check the deadline → corridor bbox → the shards that
/// cover it → a Range read of each **present** shard → corridor extraction → one frame per timeline
/// step. There is no selection step, because there is nothing to select between (#1244).
///
/// Three rules the fetch path exists to keep, all of them about the one answer that must never be
/// invented:
///
/// - **A present shard that 404s, arrives short or fails a CRC is an error.** The manifest said the
///   object exists; the object not being there is a failure to surface and retry, never an absence of
///   rain. It fails the *frame*, and only losing every frame removes the rain map.
/// - **A bitmap-absent shard is dry, and dry is painted.** Intensity 0 goes into the frame for every
///   cell of it. A fully dry frame is a real, all-zero frame in the bundle — that is how "no rain"
///   renders, and it is why nothing here treats an empty fetch list as "nothing to show".
/// - **Expired, out-of-domain, uncovered and unreachable are not dry maps.** Each degrades to its own
///   ``NoRainMapReason``, and the hourly forecast is untouched by all four.
public actor OBCWeatherServiceClient: PrecipitationGridProvider, WeatherServiceStatusProviding {
    /// The manifest key from OBCG §10.
    public static let manifestKey = WeatherManifestV2.manifestKey
    /// How long a manifest is reused before the document's own `manifest_max_age_s` is known — one
    /// fetch's worth of caution, replaced by the document's number the moment one has been read.
    public static let manifestFreshnessWindow: TimeInterval = 60

    private struct ManifestCache {
        var manifest: WeatherManifestV2
        var entityTag: String?
        var fetchedAt: Date
    }

    private let baseURL: URL
    private let client: any WeatherHTTPClient
    private let cache: any WeatherFrameCache
    private var manifestCache: ManifestCache?

    public init(
        baseURL: URL, client: any WeatherHTTPClient,
        cache: any WeatherFrameCache = InMemoryWeatherFrameCache()
    ) {
        self.baseURL = baseURL
        self.client = client
        self.cache = cache
    }

    // MARK: - PrecipitationGridProvider

    public func precipitation(
        for corridor: WeatherCorridor, now: Date
    ) async throws -> PrecipitationOutcome {
        var diagnostics = WeatherDiagnostics()
        let manifest: WeatherManifestV2
        do {
            manifest = try await self.manifest(now: now, diagnostics: &diagnostics)
        } catch {
            return .unavailable(.serviceUnavailable, diagnostics)
        }
        diagnostics.clockSkewSuspected = manifest.clockSkewSuspected(at: now)
        diagnostics.skippedManifestFrames = manifest.skippedFrames

        let plan = manifest.plan(bbox: corridor.bounds, now: now)
        switch plan.outcome {
        case .outOfDomain: return .unavailable(.outOfDomain, diagnostics)
        case .uncovered: return .unavailable(.uncovered, diagnostics)
        case .expired:
            return .unavailable(.expired(staleAfter: manifest.freshness.staleAfter), diagnostics)
        case .covered: break
        }

        // The frame window is the corridor on the lattice, derived from the same arithmetic the
        // shard set is, so the shards planned for cover exactly the cells the frame declares.
        guard let window = manifest.lattice.cellWindow(for: corridor.bounds) else {
            return .unavailable(.outOfDomain, diagnostics)
        }

        var crops: [PrecipitationCrop] = []
        for frame in manifest.frames {
            // Frames outside the usable window are not fetched: two hours ahead is the question the
            // rain map answers, and an observation older than six hours would be a lie told with a
            // true timestamp. Both are properties of the *timeline*, not of any product, which is
            // why the two constants live on ``WeatherCorridor`` beside the radius.
            guard frame.validAt <= now.addingTimeInterval(WeatherCorridor.horizon),
                  frame.validAt >= now.addingTimeInterval(-WeatherCorridor.maximumObservationAge)
            else {
                diagnostics.framesOutsideWindow += 1
                continue
            }
            let reads = plan.fetch.filter { $0.offsetMinutes == frame.offsetMinutes }
            let dry = plan.dry.filter { $0.offsetMinutes == frame.offsetMinutes }
            if let crop = await self.crop(
                frame: frame, reads: reads, dry: dry, window: window,
                lattice: manifest.lattice, cadence: manifest.cadence, now: now,
                diagnostics: &diagnostics) {
                crops.append(crop)
            }
        }
        guard !crops.isEmpty else {
            // Nothing failed and nothing is left: every frame this generation publishes is about a
            // different time. Saying "the frames couldn't be downloaded" there would blame a
            // service that answered perfectly, so it gets its own sentence.
            if diagnostics.failedShards == 0, diagnostics.framesOutsideWindow > 0 {
                return .unavailable(.outsideWindow, diagnostics)
            }
            return .unavailable(.framesUnavailable, diagnostics)
        }

        return .selected(
            PrecipitationSelection(
                generation: manifest.generation,
                nominalCellMetres: manifest.lattice.cellSizeMetres,
                attributions: manifest.attributions, referenceTime: manifest.referenceTime,
                generatedAt: manifest.generatedAt,
                stalenessDeadline: manifest.freshness.staleAfter, crops: crops),
            diagnostics)
    }

    /// Revalidate only the small mutable manifest. No shard Range request is made unless the caller
    /// subsequently decides that the revision requires a full bundle build.
    public func currentRevision(now: Date) async throws -> PrecipitationRevision? {
        var diagnostics = WeatherDiagnostics()
        let manifest = try await self.manifest(now: now, diagnostics: &diagnostics)
        guard manifest.freshness.isUsable(at: now) else { return nil }
        return PrecipitationRevision(
            generation: manifest.generation,
            generatedAt: manifest.generatedAt,
            nextGenerationExpectedAt: manifest.freshness.nextGenerationExpectedAt)
    }

    // MARK: - WeatherServiceStatusProviding

    /// The manifest as health + credits (WX13). Rides the same manifest cache the corridor path uses,
    /// so opening the weather screen right after a job costs nothing, and carries no corridor or
    /// coordinate of its own.
    public func serviceStatus(now: Date) async throws -> WeatherServiceStatus {
        var diagnostics = WeatherDiagnostics()
        let manifest = try await self.manifest(now: now, diagnostics: &diagnostics)
        return WeatherServiceStatus(manifest: manifest, observedAt: manifestCache?.fetchedAt ?? now)
    }

    // MARK: - Manifest

    private func manifest(
        now: Date, diagnostics: inout WeatherDiagnostics
    ) async throws -> WeatherManifestV2 {
        // The document states its own cache lifetime, so the client holds no interval of its own
        // past the first read: the service can change the cadence without an app release.
        if let cached = manifestCache, now >= cached.fetchedAt,
           !cached.manifest.freshness.manifestIsStale(fetchedAt: cached.fetchedAt, now: now) {
            return cached.manifest
        }
        let url = baseURL.appendingPathComponent(Self.manifestKey)
        let response: WeatherHTTPResponse
        do {
            response = try await client.perform(
                WeatherHTTPRequest(url: url, entityTag: manifestCache?.entityTag))
        } catch {
            // Offline with a manifest in hand is not an outage yet: the generation carries its own
            // `stale_after`, so reusing the document can only ever answer inside a deadline the
            // document itself still declares usable — and `plan` refuses past it.
            if let cached = manifestCache { return cached.manifest }
            throw WeatherManifestError.malformed
        }
        diagnostics.serviceRequests += 1
        diagnostics.serviceBytes += response.body.count

        if response.isNotModified, let cached = manifestCache {
            manifestCache?.fetchedAt = now
            return cached.manifest
        }
        guard response.isSuccess else {
            if let cached = manifestCache { return cached.manifest }
            throw WeatherManifestError.malformed
        }
        let parsed = try WeatherManifestV2.parse(response.body)
        manifestCache = ManifestCache(
            manifest: parsed, entityTag: response.header("ETag"), fetchedAt: now)
        return parsed
    }

    // MARK: - Corridor extraction

    /// Assemble one timeline step from up to four shard crops.
    ///
    /// A 90 km disc straddles a shard seam routinely — the shard grid is 6,144 x 4,608 cells, but a
    /// corridor can sit on a corner — so a frame is a *mosaic*: each present shard contributes the
    /// part of the window it covers, each dry shard contributes intensity 0, and any cell no shard
    /// reaches stays no-data and raises the partial-coverage flag.
    ///
    /// **A failed shard is a hole in its frame, not the loss of the frame.** The manifest promised
    /// the object, so its absence is an error and is counted — but the eight shards that did arrive
    /// are not thrown away to punish the one that did not. The hole stays no-data, which is
    /// distinguishable from dry at every layer below, so keeping it cannot make an outage look
    /// rain-free; it also raises the partial-coverage flag. `nil` — the frame disappearing
    /// entirely — happens only when *every* present shard failed and nothing was dry, because that
    /// frame would be an all-no-data image claiming a timestamp.
    private func crop(
        frame: WeatherManifestFrame, reads: [WeatherPlannedRead], dry: [WeatherFrameShard],
        window: WeatherCellWindow, lattice: WeatherLattice, cadence: WeatherCadence, now: Date,
        diagnostics: inout WeatherDiagnostics
    ) async -> PrecipitationCrop? {
        var cells = [UInt8](repeating: OBCPrecipitationTileCodec.noData,
                            count: window.width * window.height)
        var known = 0

        for entry in dry {
            // Dry is *painted*, not skipped. A shard the baker measured as dry everywhere is
            // intensity 0 in every one of its cells, and a frame made only of those is a real
            // all-zero frame — which is how "no rain" reaches the glass.
            guard let local = shardWindow(shard: entry.shard, window: window, lattice: lattice)
            else { continue }
            diagnostics.dryShards += 1
            known += 1
            paint(&cells, window: window, region: local.region) { _, _ in
                OBCPrecipitationTileCodec.dry
            }
        }

        for read in reads {
            guard let local = shardWindow(shard: read.shard, window: window, lattice: lattice)
            else { continue }
            if read.observed { diagnostics.observedShards += 1 }
            do {
                let shardCells = try await shardCrop(
                    read: read, frame: frame, geometry: local.geometry,
                    cellWindow: local.cellWindow, diagnostics: &diagnostics)
                paint(&cells, window: window, region: local.region) { column, row in
                    shardCells[row * local.cellWindow.width + column]
                }
                known += 1
            } catch {
                // Counted, and left as no-data. Never painted dry, and never allowed to remove the
                // shards that did arrive.
                diagnostics.failedShards += 1
            }
        }
        guard known > 0 else { return nil }

        // **The quality flag follows the frame's temporal nature**, not its content and not the
        // per-shard `observed` bits. An OBCW frame carries one flag for a mosaic that is radar over
        // the rider and model fill across the seam, so no content rule can be true of all of it —
        // and a content rule made an all-dry frame's flag depend on whether the baker happened to
        // publish an object, which is how the two clients came to disagree about the commonest
        // scene there is. So: the frame at offset 0 whose validity is within the dataset's own
        // `max_source_skew_s` of now is the analysis and says observed; every forward frame is a
        // forecast and says so. An all-dry radar scan is still an observation; an all-dry forecast
        // frame is still a forecast. The per-shard bits stay in the diagnostics, where they are true.
        let skew = Swift.max(0, cadence.maximumSourceSkew)
        let observed = frame.offsetMinutes == 0
            && abs(now.timeIntervalSince(frame.validAt)) <= skew
        // Partial coverage is decided over the **assembled** frame, and over nothing else:
        // `OBCW_Spec.md` §5.1 defines it as some *in-bounds* cell being unavailable, and in-bounds
        // means inside the window this bundle states. A corridor clamped to the lattice edge — at
        // the date line, or against a lattice that does not reach the rider — produces a smaller
        // window whose every cell has data, so raising the flag there would tell the device that
        // cells it can see are unknown when all of them are known. What the rider lost is window,
        // not certainty, and `window.isClipped` stays evidence for the diagnostics rather than a
        // claim about the frame. Rust decides it the same way, in `bundle::rain_frame`.
        var quality: PrecipitationQuality = observed ? .observed : .forecast
        if cells.contains(OBCPrecipitationTileCodec.noData) { quality.insert(.partialCoverage) }

        return PrecipitationCrop(
            validAt: frame.validAt,
            southMicrodegrees: Int64(lattice.southLatitudeMicrodegrees)
                + Int64(window.rowMinimum) * Int64(lattice.cellMicrodegrees),
            westMicrodegrees: Int64(lattice.westLongitudeMicrodegrees)
                + Int64(window.columnMinimum) * Int64(lattice.cellMicrodegrees),
            latitudeStrideMicrodegrees: lattice.cellMicrodegrees,
            longitudeStrideMicrodegrees: lattice.cellMicrodegrees,
            width: window.width, height: window.height,
            cellSizeMetres: lattice.cellSizeMetres, quality: quality, cells: cells)
    }

    /// Where one shard meets the frame window: the shard's OBCG geometry, the window inside that
    /// shard to read, and where those cells land in the frame. `nil` when they do not overlap.
    private func shardWindow(
        shard: WeatherShardID, window: WeatherCellWindow, lattice: WeatherLattice
    ) -> (geometry: WeatherFrameGeometry, cellWindow: CorridorExtraction.CellWindow,
          region: (column: Int, row: Int, width: Int, height: Int))? {
        guard let geometry = lattice.geometry(of: shard) else { return nil }
        let originColumn = Int(shard.column * lattice.shardWidth)
        let originRow = Int(shard.row * lattice.shardHeight)
        let firstColumn = Swift.max(window.columnMinimum, originColumn)
        let firstRow = Swift.max(window.rowMinimum, originRow)
        let lastColumn = Swift.min(
            window.columnMinimum + window.width, originColumn + Int(geometry.width)) - 1
        let lastRow = Swift.min(
            window.rowMinimum + window.height, originRow + Int(geometry.height)) - 1
        guard firstColumn <= lastColumn, firstRow <= lastRow else { return nil }
        return (
            geometry,
            CorridorExtraction.CellWindow(
                columnMinimum: UInt32(firstColumn - originColumn),
                rowMinimum: UInt32(firstRow - originRow),
                width: lastColumn - firstColumn + 1, height: lastRow - firstRow + 1,
                isClipped: false),
            (firstColumn - window.columnMinimum, firstRow - window.rowMinimum,
             lastColumn - firstColumn + 1, lastRow - firstRow + 1))
    }

    private func paint(
        _ cells: inout [UInt8], window: WeatherCellWindow,
        region: (column: Int, row: Int, width: Int, height: Int),
        value: (Int, Int) -> UInt8
    ) {
        for row in 0..<region.height {
            for column in 0..<region.width {
                cells[(region.row + row) * window.width + region.column + column] =
                    value(column, row)
            }
        }
    }

    /// Fetch and decode exactly one shard's part of one frame: header, covering directory pages,
    /// needed non-dry tiles. Every page and tile is verified against its own CRC before a single cell
    /// of it is believed, and the whole object is checked against what the manifest promised.
    private func shardCrop(
        read: WeatherPlannedRead, frame: WeatherManifestFrame, geometry: WeatherFrameGeometry,
        cellWindow window: CorridorExtraction.CellWindow, diagnostics: inout WeatherDiagnostics
    ) async throws -> [UInt8] {
        let cacheKey = WeatherFrameCacheKey(
            objectKey: read.key, columnMinimum: window.columnMinimum,
            rowMinimum: window.rowMinimum, width: window.width, height: window.height)
        if let cached = await cache.crop(for: cacheKey) { return cached.cells }

        let url = baseURL.appendingPathComponent(read.key)
        let headerBytes = try await self.read(
            url: url, range: 0..<OBCGridCodec.headerLength, diagnostics: &diagnostics)
        let header = try OBCGridCodec.decodeHeader(headerBytes)
        // The manifest planned this read; the header must agree with the plan, and the object must
        // be the length the manifest promised. Disagreement means one of them is wrong — refuse
        // rather than decode whichever happens to be self-consistent.
        guard geometry.agrees(with: header), Int(header.totalLength) == read.byteLength,
              header.objectCRC32 == read.objectCRC32,
              // The frame's genuine validity time, agreed by both. A manifest that re-stamped a
              // frame to look current would be caught right here.
              Int64(frame.validAt.timeIntervalSince1970.rounded()) == header.validAtUnixSeconds
        else { throw OBCWeatherWireError.malformed }

        let tileIndexes = try CorridorExtraction.tileIndexes(header: header, window: window)
        var pageBytes: [Int: Data] = [:]
        for range in CorridorExtraction.coalesce(
            try CorridorExtraction.pageRanges(header: header, window: window)) {
            let bytes = try await self.read(url: url, range: range, diagnostics: &diagnostics)
            // Split a coalesced read back into whole pages: each page verifies independently.
            var offset = range.lowerBound
            while offset < range.upperBound {
                let page = (offset - OBCGridCodec.headerLength) / header.pageBytes
                guard let slice = bytes.sliced(
                    at: offset - range.lowerBound, count: header.pageBytes)
                else { throw OBCWeatherWireError.malformed }
                try OBCGridCodec.validatePage(header: header, page: slice)
                pageBytes[page] = slice
                offset += header.pageBytes
            }
        }

        // Entries first, so the tile payload ranges are known before any payload is fetched — the
        // spec's two-step, and what lets consecutive payloads coalesce into one request.
        var entries: [Int: OBCGridTileEntry] = [:]
        var payloadRanges: [Range<Int>] = []
        for index in tileIndexes {
            let page = header.pageOfEntry(index)
            guard let bytes = pageBytes[page] else { throw OBCWeatherWireError.malformed }
            let entry = try OBCGridCodec.decodeEntry(
                page: bytes, indexInPage: index - page * Int(header.entriesPerPage))
            if entry.isDry {
                // §4.1: a partial edge tile carries no-data padding and can never be a dry
                // sentinel. Accepting one would decode missing data as dry weather.
                guard !header.tileIsPartial(index) else { throw OBCWeatherWireError.malformed }
            } else {
                payloadRanges.append(try OBCGridCodec.payloadRange(header: header, entry: entry))
            }
            entries[index] = entry
        }

        var payloads: [Int: Data] = [:]
        for range in CorridorExtraction.coalesce(payloadRanges) {
            let bytes = try await self.read(url: url, range: range, diagnostics: &diagnostics)
            for index in tileIndexes {
                guard let entry = entries[index], !entry.isDry else { continue }
                let payload = try OBCGridCodec.payloadRange(header: header, entry: entry)
                guard payload.lowerBound >= range.lowerBound, payload.upperBound <= range.upperBound,
                      let slice = bytes.sliced(
                        at: payload.lowerBound - range.lowerBound, count: payload.count)
                else { continue }
                payloads[index] = slice
            }
        }

        var cells = [UInt8](repeating: OBCPrecipitationTileCodec.noData,
                            count: window.width * window.height)
        let edge = Int(header.tileEdge)
        for index in tileIndexes {
            guard let entry = entries[index] else { throw OBCWeatherWireError.malformed }
            let payload = payloads[index] ?? Data()
            let tile = try OBCGridCodec.decodeTileCells(
                header: header, entry: entry, payload: payload)
            let tileColumn = index % header.tileColumns
            let tileRow = index / header.tileColumns
            for localRow in 0..<edge {
                let row = tileRow * edge + localRow
                guard row >= Int(window.rowMinimum), row < Int(window.rowMinimum) + window.height
                else { continue }
                for localColumn in 0..<edge {
                    let column = tileColumn * edge + localColumn
                    guard column >= Int(window.columnMinimum),
                          column < Int(window.columnMinimum) + window.width else { continue }
                    cells[(row - Int(window.rowMinimum)) * window.width
                        + (column - Int(window.columnMinimum))] = tile[localRow * edge + localColumn]
                }
            }
        }

        // Cached as a crop so a second corridor over the same shard window costs no HTTP at all;
        // the geographic fields are the shard's own, which is what makes the entry self-describing.
        // So is the quality: this is one *shard*, and the manifest's per-shard `observed` bit is
        // true of it. It does not reach the frame's flag — only `crop(frame:...)` decides that, from
        // the frame's place in the timeline — and only `cells` is ever read back out of here.
        await cache.store(
            PrecipitationCrop(
                validAt: frame.validAt,
                southMicrodegrees: Int64(geometry.southMicrodegrees)
                    + Int64(window.rowMinimum) * Int64(geometry.cellLatitudeMicrodegrees),
                westMicrodegrees: Int64(geometry.westMicrodegrees)
                    + Int64(window.columnMinimum) * Int64(geometry.cellLongitudeMicrodegrees),
                latitudeStrideMicrodegrees: geometry.cellLatitudeMicrodegrees,
                longitudeStrideMicrodegrees: geometry.cellLongitudeMicrodegrees,
                width: window.width, height: window.height,
                cellSizeMetres: geometry.cellSizeMetres,
                quality: read.observed ? .observed : .forecast, cells: cells),
            for: cacheKey)
        return cells
    }

    private func read(
        url: URL, range: Range<Int>, diagnostics: inout WeatherDiagnostics
    ) async throws -> Data {
        let response = try await client.perform(WeatherHTTPRequest(url: url, byteRange: range))
        diagnostics.serviceRequests += 1
        diagnostics.serviceBytes += response.body.count
        guard response.statusCode == 206 || response.statusCode == 200 else {
            throw WeatherHTTPError.unacceptableStatus(
                code: response.statusCode, retryAfterSeconds: response.retryAfterSeconds)
        }
        // A `200` to a Range request means the whole object arrived; take the slice ourselves
        // rather than parsing the head of a file as if it were the middle.
        if response.statusCode == 200, response.body.count != range.count {
            guard let slice = response.body.sliced(at: range.lowerBound, count: range.count)
            else { throw WeatherHTTPError.rangeNotHonoured }
            return slice
        }
        guard response.body.count == range.count else { throw WeatherHTTPError.rangeNotHonoured }
        return response.body
    }
}

// Byte access local to this target, named distinctly from `OBCWeatherWire`'s internal equivalent so
// a `@testable import` of both modules stays unambiguous. Keeping a small private copy here is
// cheaper than widening the codec's public surface.
extension Data {
    func sliced(at offset: Int, count length: Int) -> Data? {
        guard offset >= 0, length >= 0 else { return nil }
        let (end, overflow) = offset.addingReportingOverflow(length)
        guard !overflow, end <= count else { return nil }
        return Data(self[(startIndex + offset)..<(startIndex + end)])
    }
}
