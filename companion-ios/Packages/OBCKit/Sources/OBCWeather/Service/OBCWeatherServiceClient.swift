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
        diagnostics.dryShards = plan.dry.count

        // The frame window is the corridor on the lattice, derived from the same arithmetic the
        // shard set is, so the shards planned for cover exactly the cells the frame declares.
        guard let window = manifest.lattice.cellWindow(for: corridor.bounds) else {
            return .unavailable(.outOfDomain, diagnostics)
        }

        var crops: [PrecipitationCrop] = []
        for frame in manifest.frames {
            let reads = plan.fetch.filter { $0.offsetMinutes == frame.offsetMinutes }
            let dry = plan.dry.filter { $0.offsetMinutes == frame.offsetMinutes }
            do {
                crops.append(try await self.crop(
                    frame: frame, reads: reads, dry: dry, window: window,
                    lattice: manifest.lattice, diagnostics: &diagnostics))
            } catch {
                // One bad frame is not a bad dataset: the rest of the timeline is still genuine, and
                // the missing timestamps show up as a shorter timeline rather than as a gap silently
                // filled in. Only losing *every* frame removes the rain map.
                continue
            }
        }
        guard !crops.isEmpty else { return .unavailable(.framesUnavailable, diagnostics) }

        return .selected(
            PrecipitationSelection(
                generation: manifest.generation,
                nominalCellMetres: manifest.lattice.cellSizeMetres,
                attributions: manifest.attributions, referenceTime: manifest.referenceTime,
                generatedAt: manifest.generatedAt,
                stalenessDeadline: manifest.freshness.staleAfter, crops: crops),
            diagnostics)
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
    /// Throwing here fails **this frame only**. It is what a present shard failing must do: the
    /// manifest promised the object, so its absence is an error, and the one thing it may never
    /// become is a hole full of zeroes.
    private func crop(
        frame: WeatherManifestFrame, reads: [WeatherPlannedRead], dry: [WeatherFrameShard],
        window: WeatherCellWindow, lattice: WeatherLattice,
        diagnostics: inout WeatherDiagnostics
    ) async throws -> PrecipitationCrop {
        var cells = [UInt8](repeating: OBCPrecipitationTileCodec.noData,
                            count: window.width * window.height)

        for entry in dry {
            // Dry is *painted*, not skipped. A shard the baker measured as dry everywhere is
            // intensity 0 in every one of its cells, and a frame made only of those is a real
            // all-zero frame — which is how "no rain" reaches the glass.
            guard let local = shardWindow(shard: entry.shard, window: window, lattice: lattice)
            else { continue }
            paint(&cells, window: window, region: local.region) { _, _ in
                OBCPrecipitationTileCodec.dry
            }
        }

        for read in reads {
            guard let local = shardWindow(shard: read.shard, window: window, lattice: lattice)
            else { continue }
            let shardCells = try await shardCrop(
                read: read, frame: frame, geometry: local.geometry, cellWindow: local.cellWindow,
                diagnostics: &diagnostics)
            paint(&cells, window: window, region: local.region) { column, row in
                shardCells[row * local.cellWindow.width + column]
            }
        }

        // Observed only when every shard that contributed data says so — one modelled shard makes
        // the frame a forecast, because the rider cannot see the seam. A frame with no present
        // shards at all is observed by the same rule, and that is right: the baker *measured* every
        // cell as dry, which is an observation of no rain rather than a prediction of it.
        var quality: PrecipitationQuality =
            reads.allSatisfy(\.observed) ? .observed : .forecast
        if window.isClipped { quality.insert(.partialCoverage) }
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
        let geometry = lattice.geometry(of: shard)
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
