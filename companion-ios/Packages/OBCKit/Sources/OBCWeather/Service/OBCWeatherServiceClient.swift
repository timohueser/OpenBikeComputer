import Foundation
import OBCWeatherWire

/// The client for the OBC weather service: one mutable manifest and a lot of immutable frame
/// objects in object storage, and nothing else.
///
/// What the service learns about a rider is the point of the design. There is no per-request
/// compute, no account and no coordinate in any URL — the only locality signal is *which tile
/// indexes* a Range header asks for inside an immutable object, and even that is a corridor rather
/// than a position. MET is the sole third party that receives an actual coordinate (WX1), and WX13
/// documents both facts to the rider.
///
/// Everything that can go wrong here degrades to a state instead of an error: an unreachable or
/// malformed manifest is a service outage, a corridor no product covers is the explicit
/// no-rain-map state, and either way the hourly forecast is untouched.
public actor OBCWeatherServiceClient: PrecipitationGridProvider {
    /// The manifest key from OBCG §10.
    public static let manifestKey = "wx/v1/manifest.json"
    /// The manifest caches for at most 60 s (OBCG §10), so that is how long this client reuses one
    /// without even a conditional request.
    public static let manifestFreshnessWindow: TimeInterval = 60

    private struct ManifestCache {
        var manifest: WeatherServiceManifest
        var entityTag: String?
        var fetchedAt: Date
        var skippedProducts: Int
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
        let manifest: WeatherServiceManifest
        do {
            manifest = try await self.manifest(now: now, diagnostics: &diagnostics)
        } catch {
            return .unavailable(.serviceUnavailable, diagnostics)
        }
        diagnostics.clockSkewSuspected = ProductSelection.clockSkewSuspected(
            manifest: manifest, now: now)
        diagnostics.skippedManifestProducts = manifestCache?.skippedProducts ?? 0

        let (outcome, expired) = ProductSelection.select(
            from: manifest, corridor: corridor, now: now)
        diagnostics.expiredCoveringProducts = expired
        guard case let .selected(product) = outcome else {
            if case let .none(reason) = outcome { return .unavailable(reason, diagnostics) }
            return .unavailable(.corridorNotCovered, diagnostics)
        }

        let frames = ProductSelection.frames(of: product, now: now)
        guard !frames.isEmpty else { return .unavailable(.noFramesInWindow, diagnostics) }

        var crops: [PrecipitationCrop] = []
        for frame in frames {
            do {
                if let crop = try await self.crop(
                    frame: frame, corridor: corridor.bounds, diagnostics: &diagnostics) {
                    crops.append(crop)
                }
            } catch {
                // One bad frame is not a bad product: the rest of the timeline is still genuine,
                // and the missing timestamps show up as incomplete coverage rather than as a gap
                // silently filled in. Only losing *every* frame removes the rain map.
                continue
            }
        }
        guard !crops.isEmpty else { return .unavailable(.framesUnavailable, diagnostics) }

        return .selected(
            PrecipitationSelection(
                productID: product.id, tier: product.tier,
                nominalCellMetres: product.nominalCellMetres, attribution: product.attribution,
                referenceTime: product.referenceTime, generatedAt: product.generatedAt,
                stalenessDeadline: product.stalenessDeadline, crops: crops),
            diagnostics)
    }

    // MARK: - Manifest

    private func manifest(
        now: Date, diagnostics: inout WeatherDiagnostics
    ) async throws -> WeatherServiceManifest {
        if let cached = manifestCache,
           now.timeIntervalSince(cached.fetchedAt) < Self.manifestFreshnessWindow,
           now >= cached.fetchedAt {
            return cached.manifest
        }
        let url = baseURL.appendingPathComponent(Self.manifestKey)
        let response: WeatherHTTPResponse
        do {
            response = try await client.perform(
                WeatherHTTPRequest(url: url, entityTag: manifestCache?.entityTag))
        } catch {
            // Offline with a manifest in hand is not an outage yet: its products carry their own
            // staleness deadlines, so reusing it can only ever select something the manifest itself
            // still declares usable.
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
        let parsed = try WeatherServiceManifest.parse(response.body)
        manifestCache = ManifestCache(
            manifest: parsed.manifest, entityTag: response.header("ETag"), fetchedAt: now,
            skippedProducts: parsed.skippedProducts)
        return parsed.manifest
    }

    // MARK: - Corridor extraction

    /// Fetch and decode exactly the corridor's part of one frame: header, covering directory pages,
    /// needed non-dry tiles. Every page and tile is verified against its own CRC before a single
    /// cell of it is believed.
    private func crop(
        frame: WeatherServiceFrame, corridor: WeatherBoundingBox,
        diagnostics: inout WeatherDiagnostics
    ) async throws -> PrecipitationCrop? {
        guard let window = CorridorExtraction.window(geometry: frame.geometry, corridor: corridor)
        else { return nil }
        let cacheKey = WeatherFrameCacheKey(
            objectKey: frame.key, columnMinimum: window.columnMinimum,
            rowMinimum: window.rowMinimum, width: window.width, height: window.height)
        if let cached = await cache.crop(for: cacheKey) { return cached }

        let url = baseURL.appendingPathComponent(frame.key)
        let headerBytes = try await read(url: url, range: 0..<OBCGridCodec.headerLength,
                                         diagnostics: &diagnostics)
        let header = try OBCGridCodec.decodeHeader(headerBytes)
        // The manifest planned this read; the header must agree with the plan, and the object must
        // be the length the manifest promised. Disagreement means one of them is wrong — refuse
        // rather than decode whichever happens to be self-consistent.
        guard frame.geometry.agrees(with: header), Int(header.totalLength) == frame.byteLength,
              header.objectCRC32 == frame.objectCRC32,
              // The frame's genuine validity time, agreed by both. A manifest that re-stamped a
              // frame to look current would be caught right here.
              Int64(frame.validAt.timeIntervalSince1970.rounded()) == header.validAtUnixSeconds
        else { throw OBCWeatherWireError.malformed }

        let tileIndexes = try CorridorExtraction.tileIndexes(header: header, window: window)
        var pageBytes: [Int: Data] = [:]
        for range in CorridorExtraction.coalesce(
            try CorridorExtraction.pageRanges(header: header, window: window)) {
            let bytes = try await read(url: url, range: range, diagnostics: &diagnostics)
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
            let bytes = try await read(url: url, range: range, diagnostics: &diagnostics)
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

        var quality: PrecipitationQuality = frame.sourceClass == .observation ? .observed : .forecast
        if window.isClipped { quality.insert(.partialCoverage) }
        if cells.contains(OBCPrecipitationTileCodec.noData) { quality.insert(.partialCoverage) }

        let crop = PrecipitationCrop(
            validAt: frame.validAt,
            southMicrodegrees: Int64(frame.geometry.southMicrodegrees)
                + Int64(window.rowMinimum) * Int64(frame.geometry.cellLatitudeMicrodegrees),
            westMicrodegrees: Int64(frame.geometry.westMicrodegrees)
                + Int64(window.columnMinimum) * Int64(frame.geometry.cellLongitudeMicrodegrees),
            latitudeStrideMicrodegrees: frame.geometry.cellLatitudeMicrodegrees,
            longitudeStrideMicrodegrees: frame.geometry.cellLongitudeMicrodegrees,
            width: window.width, height: window.height,
            cellSizeMetres: frame.geometry.cellSizeMetres, quality: quality, cells: cells)
        await cache.store(crop, for: cacheKey)
        return crop
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
