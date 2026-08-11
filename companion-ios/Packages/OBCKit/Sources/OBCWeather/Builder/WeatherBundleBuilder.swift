import Foundation
import OBCDomain
import OBCWeatherWire

public enum WeatherBundleBuildError: Error, Equatable, Sendable {
    /// The hourly section is the one part a bundle cannot be built without. 24 consecutive hours or
    /// nothing — padding the gap would put invented weather on a device that cannot tell.
    case hourlyUnusable
    /// A corridor with no positive extent; nothing to describe.
    case invalidBounds
    /// Even a single frame and the smallest window exceeded the producer policy.
    case tooLarge
}

/// One finished bundle: the bytes to upload, the decoded value they encode, and the semantic state
/// the app keeps for its own screens.
public struct BuiltWeatherBundle: Equatable, Sendable {
    public var bytes: Data
    public var bundle: OBCWeatherBundle
    public var state: WeatherState

    public init(bytes: Data, bundle: OBCWeatherBundle, state: WeatherState) {
        self.bytes = bytes
        self.bundle = bundle
        self.state = state
    }
}

/// Merges the two independent halves — MET's hourly forecast and the corridor's precipitation — into
/// one OBCW object.
///
/// Three rules shape everything here:
///
/// 1. **The halves are independent.** A missing, degraded or expired rain dataset never discards a
///    valid hourly forecast; a missing hourly forecast is a failed job, because there is no bundle
///    without it.
/// 2. **The one resample is the east–west one, and it is stated.** Crops arrive on the dataset's
///    single 0.01° lattice; rows are copied 1:1 and columns are decimated onto a uniform ~1,113 m
///    pitch by nearest neighbour (see ``CommonWindow/outputColumns``). Nothing else is resampled,
///    interpolated or smoothed.
/// 3. **The output is deterministic.** Same inputs, byte-identical bundle — which is what makes the
///    golden test meaningful and what lets the device's generation/CRC comparison mean something.
public struct WeatherBundleBuilder: Sendable {
    /// How many times the common window may be shrunk before frames start being dropped. Each step
    /// removes an eighth of each axis, so this reaches a single tile long before it runs out.
    ///
    /// With the uniform-pitch resample below the shrink loop is a **backstop**, not the normal path:
    /// #1254 measured 162 x 162 cells at every latitude from the equator to 80 °N, whose raw4 worst
    /// case is 153.6 kB against a 256 KiB cap. The loop stays for the case that measurement does not
    /// cover.
    public static let maximumShrinkAttempts = 24

    public init() {}

    public func build(
        request: WeatherRequest,
        corridor: WeatherCorridor,
        hourly: HourlyForecast,
        precipitation: PrecipitationSelection?,
        noRainMapReason: NoRainMapReason?,
        generation: UInt32,
        now: Date,
        diagnostics: WeatherDiagnostics = WeatherDiagnostics()
    ) throws -> BuiltWeatherBundle {
        let hours = try hourlyRecords(hourly)
        let validFrom = Int64(hours.validFrom.timeIntervalSince1970.rounded())
        var diagnostics = diagnostics

        var crops = precipitation?.crops ?? []
        crops.sort { $0.validAt < $1.validAt }
        // OBCW §5 requires strictly increasing frame timestamps; a duplicate timestamp is a
        // producer contradiction, so the later one is dropped rather than reordered into fiction.
        crops = crops.reduce(into: [PrecipitationCrop]()) { result, crop in
            if result.last?.validAt != crop.validAt { result.append(crop) }
        }

        var bounds = try commonWindow(crops: crops, corridor: corridor, rider: request.position)
        bounds = budgeted(bounds, crops: crops)
        var frames: [OBCWeatherRainFrame] = []
        var encoded: Data?

        var attempt = 0
        var usable = crops
        while true {
            frames = usable.compactMap { rainFrame(crop: $0, window: bounds) }
            let candidate = OBCWeatherBundle(
                generation: generation, requestID: request.requestID,
                generatedAtUnixSeconds: Int64(now.timeIntervalSince1970.rounded()),
                validFromUnixSeconds: validFrom,
                validUntilUnixSeconds: validUntil(validFrom: validFrom, frames: frames),
                bounds: bounds.obcwBounds, hourly: hours.records, rainFrames: frames)
            do {
                encoded = try OBCWeatherCodec.encode(candidate)
                break
            } catch OBCWeatherWireError.producerPolicyExceeded {
                // Too big for the producer policy. Trim the *window* first — a slightly
                // shorter corridor still answers the two-hour question at every timestamp, while
                // dropping frames puts holes in the timeline. Only once the window cannot usefully
                // shrink do the furthest-future frames go, and both facts are reported.
                if attempt < Self.maximumShrinkAttempts, let shrunk = bounds.shrunk() {
                    bounds = shrunk
                    attempt += 1
                    continue
                }
                guard usable.count > 1 else { throw WeatherBundleBuildError.tooLarge }
                usable.removeLast()
                diagnostics.droppedOversizeFrames += 1
                attempt = 0
                continue
            }
        }
        guard let bytes = encoded else { throw WeatherBundleBuildError.tooLarge }

        var selection = precipitation
        if frames.isEmpty { selection = nil }
        let attributions = ([hourly.attribution] + (selection?.attributions ?? []))
            .reduce(into: [WeatherAttribution]()) { unique, entry in
                if !unique.contains(entry) { unique.append(entry) }
            }
        let state = WeatherState(
            hourly: hourly, precipitation: selection,
            noRainMapReason: selection == nil ? (noRainMapReason ?? .framesUnavailable) : nil,
            attributions: attributions, diagnostics: diagnostics)
        return BuiltWeatherBundle(
            bytes: bytes, bundle: try OBCWeatherCodec.decode(bytes), state: state)
    }

    // MARK: - Hourly

    private struct HourlySection {
        var validFrom: Date
        var records: [OBCWeatherHourlyRecord]
    }

    /// Translate 24 semantic hours into the fixed wire records.
    ///
    /// Every unavailable value becomes its explicit sentinel and **never** a zero: OBCW §4 says so,
    /// and the difference is "we do not know whether it will rain" versus "it will not rain". A
    /// value outside the wire's range is treated as unavailable rather than clamped, because a
    /// clamped 80 m/s gust is a number the rider would believe.
    private func hourlyRecords(_ forecast: HourlyForecast) throws -> HourlySection {
        guard forecast.hours.count == METLocationforecastAdapter.requiredHours,
              let first = forecast.hours.first
        else { throw WeatherBundleBuildError.hourlyUnusable }
        for index in 1..<forecast.hours.count {
            guard forecast.hours[index].validAt
                .timeIntervalSince(forecast.hours[index - 1].validAt) == 3_600
            else { throw WeatherBundleBuildError.hourlyUnusable }
        }
        let records = forecast.hours.enumerated().map { index, hour in
            OBCWeatherHourlyRecord(
                validTimeOffsetSeconds: UInt32(index) * 3_600,
                temperatureDeciCelsius: scaledInt16(
                    hour.temperatureCelsius, scale: 10, range: -1_000...700),
                precipitationTenthMillimetres: scaledUInt16(
                    hour.precipitationMillimetres, scale: 10, maximum: 65_534),
                precipitationProbabilityPercent: percent(hour.precipitationProbabilityPercent),
                condition: hour.condition,
                windFromDegrees: degrees(hour.windFromDegrees),
                windSpeedDeciMetresPerSecond: scaledUInt16(
                    hour.windSpeedMetresPerSecond, scale: 10, maximum: 2_000),
                windGustDeciMetresPerSecond: scaledUInt16(
                    hour.windGustMetresPerSecond, scale: 10, maximum: 2_000))
        }
        return HourlySection(validFrom: first.validAt, records: records)
    }

    private func scaledInt16(_ value: Double?, scale: Double, range: ClosedRange<Int16>) -> Int16 {
        guard let value, value.isFinite else { return Int16.min }
        let scaled = (value * scale).rounded()
        guard scaled >= Double(range.lowerBound), scaled <= Double(range.upperBound) else {
            return Int16.min
        }
        return Int16(scaled)
    }

    private func scaledUInt16(_ value: Double?, scale: Double, maximum: UInt16) -> UInt16 {
        guard let value, value.isFinite, value >= 0 else { return UInt16.max }
        let scaled = (value * scale).rounded()
        guard scaled <= Double(maximum) else { return UInt16.max }
        return UInt16(scaled)
    }

    private func percent(_ value: Double?) -> UInt8 {
        guard let value, value.isFinite, value >= 0, value <= 100 else { return UInt8.max }
        return UInt8(value.rounded())
    }

    private func degrees(_ value: Double?) -> UInt16 {
        guard let value, value.isFinite else { return UInt16.max }
        // 360 is 0; a negative or over-turn bearing folds into the circle rather than being lost.
        let folded = value.truncatingRemainder(dividingBy: 360)
        let normalized = (folded < 0 ? folded + 360 : folded).rounded()
        return UInt16(normalized == 360 ? 0 : normalized)
    }

    private func validUntil(validFrom: Int64, frames: [OBCWeatherRainFrame]) -> Int64 {
        // The ceiling every hourly interval end and every rain timestamp must sit under. The hourly
        // section fixes the floor at +24 h; a forecast frame beyond that (there should be none, the
        // horizon is two hours) still raises it rather than being silently dropped at encode time.
        Swift.max(validFrom + 24 * 3_600, frames.map(\.validAtUnixSeconds).max() ?? Int64.min)
    }


    // MARK: - Rain frames

    /// The common `[south, north) x [west, east)` window every frame shares.
    ///
    /// Two grids in one value, and keeping them apart is the whole of #1254's normalisation. The
    /// **source** grid is the dataset's 0.01° lattice, whole cells, corners lattice-aligned — that is
    /// what `south`/`west`/`cellMicrodegrees`/`sourceColumns`/`rows` state, and it is what a shrink
    /// operates on so the corners stay integers. The **output** grid has the same rows and
    /// ``outputColumns`` columns spread evenly across the same longitude span, which is what the
    /// frame descriptors declare.
    struct CommonWindow: Equatable {
        var south: Int64
        var west: Int64
        /// One lattice cell, both axes. The dataset is square in degrees, so there is one number.
        var cellMicrodegrees: Int64
        /// Columns of the **source** lattice inside the window.
        var sourceColumns: Int
        /// Rows, which the resample leaves alone: the north–south pitch is already ~1,113 m.
        var rows: Int
        /// The source cell the rider sits in, so shrinking keeps them inside the window.
        var anchorColumn: Int
        var anchorRow: Int
        /// The latitude the resample is computed at — the rider's, or the corridor's midpoint when
        /// there is no fix. **The true cosine, not the 0.05-clamped one** ``WeatherBoundingBox``
        /// grows a corridor with: that clamp exists to stop a disc exploding near a pole, and reusing
        /// it here would leave twenty times too many columns at 87 °N.
        var anchorLatitudeDegrees: Double

        var north: Int64 { south + Int64(rows) * cellMicrodegrees }
        var east: Int64 { west + Int64(sourceColumns) * cellMicrodegrees }

        /// The east–west column count after the uniform-pitch resample.
        ///
        /// A 0.01° column is `1,113 * cos φ` metres wide — 715 m at 50 °N, 387 m at Tromsø — finer
        /// than anything any source produces, so a corridor of fixed *ground* radius costs more and
        /// more columns the further north the rider is, for detail that does not exist. Taking
        /// `round(sourceColumns * cos φ)` columns puts the output pitch back on the lattice's own
        /// north–south cell height, ~1,113 m, at every latitude: a 90 km disc is then 162 x 162 cells
        /// everywhere, cells are square to within 0.4 %, and `cell_size_m` is simply true.
        ///
        /// It **bounds** the cost rather than reducing it — 162 x 162 is what a 180 km box already
        /// costs at the equator — and what it removes is the unbounded northward growth, and with it
        /// the cap ladder (a 256 KiB cap tops out at 55.8 °N, 512 KiB at 74.15 °N).
        var outputColumns: Int {
            let cosine = Foundation.cos(anchorLatitudeDegrees * .pi / 180)
            let scaled = Int((Double(sourceColumns) * cosine).rounded())
            return Swift.max(1, Swift.min(sourceColumns, scaled))
        }

        var obcwBounds: OBCWeatherBounds {
            OBCWeatherBounds(
                southLatitudeMicrodegrees: Int32(south), westLongitudeMicrodegrees: Int32(west),
                northLatitudeMicrodegrees: Int32(north), eastLongitudeMicrodegrees: Int32(east),
                gridOriginLatitudeMicrodegrees: Int32(south),
                gridOriginLongitudeMicrodegrees: Int32(west))
        }

        /// The source column one output column samples — **integer arithmetic, no floats**, so the
        /// Rust and Swift clients cannot drift on a rounding mode.
        ///
        /// `floor(((2j + 1) * srcCols) / (2 * outCols))` is the centre of output column `j` mapped
        /// back onto the source and truncated: the nearest-neighbour rule `OBCG_Spec` §6 and
        /// `OBCW_Spec` §5 already mandate everywhere else. At the equator `outCols == srcCols` and it
        /// is the identity.
        static func sourceColumn(output: Int, sourceColumns: Int, outputColumns: Int) -> Int {
            Swift.min(((2 * output + 1) * sourceColumns) / (2 * outputColumns), sourceColumns - 1)
        }

        /// Remove an eighth of each axis **in source cells**, keeping the rider's cell inside.
        /// Returns `nil` once the window is a single cell in either direction and cannot usefully
        /// shrink again. Trimming source cells rather than output columns is what keeps the window's
        /// corners lattice-aligned integers across every attempt.
        ///
        /// The rider is deliberately not a parameter: `anchorColumn`/`anchorRow` **are** the rider's
        /// cell, carried through every resize. An earlier version took a `WeatherRequest` here and
        /// never read it, which read as "shrinks towards the rider" while the anchor was really the
        /// corridor's midpoint — for a rider at the back of a fast corridor that shrank the window
        /// clean off them, leaving a bundle with no rain data where they actually were.
        func shrunk() -> CommonWindow? {
            guard sourceColumns > 1 || rows > 1 else { return nil }
            return resized(
                sourceColumns: Swift.max(1, sourceColumns - Swift.max(1, sourceColumns / 8)),
                rows: Swift.max(1, rows - Swift.max(1, rows / 8)))
        }

        /// Re-centre the window on the rider at a smaller size, in whole cells of the source lattice.
        func resized(sourceColumns newColumns: Int, rows newRows: Int) -> CommonWindow {
            var resized = self
            let keptColumns = Swift.max(1, Swift.min(sourceColumns, newColumns))
            let keptRows = Swift.max(1, Swift.min(rows, newRows))
            let west = Swift.min(
                Swift.max(0, anchorColumn - keptColumns / 2), sourceColumns - keptColumns)
            let south = Swift.min(Swift.max(0, anchorRow - keptRows / 2), rows - keptRows)
            resized.west += Int64(west) * cellMicrodegrees
            resized.south += Int64(south) * cellMicrodegrees
            resized.sourceColumns = keptColumns
            resized.rows = keptRows
            resized.anchorColumn = anchorColumn - west
            resized.anchorRow = anchorRow - south
            return resized
        }
    }

    /// The window every frame is expressed over: the corridor, rounded outward to whole lattice
    /// cells, intersected with the data actually in hand.
    ///
    /// With one dataset there is nothing to choose between — no coarsest-lattice ranking, no nesting
    /// question, and no frame that can fail to tile the result — so this is arithmetic on the
    /// corridor and the lattice the crops arrived on. With no crops at all the window is the corridor
    /// itself, so an hourly-only bundle still states the region it describes.
    ///
    /// - Parameter rider: the rider's own position, which becomes the window's anchor (and therefore
    ///   the point every later shrink keeps inside) and the latitude the resample is computed at.
    ///   The corridor's midpoint is only the fallback for a request with no fix.
    private func commonWindow(
        crops: [PrecipitationCrop], corridor: WeatherCorridor, rider: Coordinate?
    ) throws -> CommonWindow {
        let corridorBounds = corridor.bounds
        guard corridorBounds.isWellFormed else { throw WeatherBundleBuildError.invalidBounds }
        // The rider's own cell when there is a fix; the corridor's midpoint only as a fallback.
        let anchorLatitude: Int64
        let anchorLongitude: Int64
        if let rider, rider.isValidGeographic {
            anchorLatitude = rider.latitudeMicrodegrees
            anchorLongitude = rider.longitudeMicrodegrees
        } else {
            anchorLatitude = (corridorBounds.southMicrodegrees + corridorBounds.northMicrodegrees) / 2
            anchorLongitude = (corridorBounds.westMicrodegrees + corridorBounds.eastMicrodegrees) / 2
        }

        guard let first = crops.first else {
            return CommonWindow(
                south: corridorBounds.southMicrodegrees, west: corridorBounds.westMicrodegrees,
                cellMicrodegrees: Swift.max(
                    corridorBounds.northMicrodegrees - corridorBounds.southMicrodegrees,
                    corridorBounds.eastMicrodegrees - corridorBounds.westMicrodegrees),
                sourceColumns: 1, rows: 1, anchorColumn: 0, anchorRow: 0,
                anchorLatitudeDegrees: Double(anchorLatitude) / 1_000_000)
        }
        // One dataset, one lattice: every crop shares this cell size and this alignment, so a window
        // derived from the first tiles all of them exactly. That is why the four remainder guards
        // this function used to hand `rainFrame` are gone rather than relaxed — there is no longer a
        // frame that can fail to tile the window, so there is no longer a frame to drop.
        let cell = Int64(first.latitudeStrideMicrodegrees)
        guard cell > 0, first.longitudeStrideMicrodegrees > 0 else {
            throw WeatherBundleBuildError.invalidBounds
        }
        // Round the corridor outward to whole lattice cells, anchored on the lattice itself, so the
        // window's corners are lattice-aligned integers in both languages.
        var south = floorDivide(corridorBounds.southMicrodegrees - first.southMicrodegrees, cell)
            * cell + first.southMicrodegrees
        var north = ceilDivide(corridorBounds.northMicrodegrees - first.southMicrodegrees, cell)
            * cell + first.southMicrodegrees
        var west = floorDivide(corridorBounds.westMicrodegrees - first.westMicrodegrees, cell)
            * cell + first.westMicrodegrees
        var east = ceilDivide(corridorBounds.eastMicrodegrees - first.westMicrodegrees, cell)
            * cell + first.westMicrodegrees
        // Intersect with the data in hand. Every crop is the same corridor window on the same
        // lattice, so this only ever trims where the corridor reached past what the shards covered —
        // and cells nobody covered would decode to no-data anyway.
        let extent = crops.dropFirst().reduce(first.bounds) { union, crop in
            let bounds = crop.bounds
            return WeatherBoundingBox(
                southMicrodegrees: Swift.min(union.southMicrodegrees, bounds.southMicrodegrees),
                westMicrodegrees: Swift.min(union.westMicrodegrees, bounds.westMicrodegrees),
                northMicrodegrees: Swift.max(union.northMicrodegrees, bounds.northMicrodegrees),
                eastMicrodegrees: Swift.max(union.eastMicrodegrees, bounds.eastMicrodegrees))
        }
        south = Swift.max(south, extent.southMicrodegrees)
        north = Swift.min(north, extent.northMicrodegrees)
        west = Swift.max(west, extent.westMicrodegrees)
        east = Swift.min(east, extent.eastMicrodegrees)
        guard south < north, west < east else { throw WeatherBundleBuildError.invalidBounds }

        let sourceColumns = Int((east - west) / cell)
        let rows = Int((north - south) / cell)
        let anchorColumn = Int(Swift.max(0, Swift.min(
            Int64(sourceColumns) - 1, floorDivide(anchorLongitude - west, cell))))
        let anchorRow = Int(Swift.max(0, Swift.min(
            Int64(rows) - 1, floorDivide(anchorLatitude - south, cell))))
        return CommonWindow(
            south: south, west: west, cellMicrodegrees: cell, sourceColumns: sourceColumns,
            rows: rows, anchorColumn: anchorColumn, anchorRow: anchorRow,
            anchorLatitudeDegrees: Double(anchorLatitude) / 1_000_000)
    }

    /// Trim the window to a size that has a chance of fitting the producer policy, using the worst
    /// case (every tile raw4) over the **output** grid, which is what actually gets encoded.
    ///
    /// This is an optimisation of the shrink loop, not a second policy: the loop is still what
    /// decides, and real RLE4 payloads are far smaller than this estimate. Without it, an oversize
    /// corridor would encode nine full frames two dozen times before converging.
    private func budgeted(_ window: CommonWindow, crops: [PrecipitationCrop]) -> CommonWindow {
        guard !crops.isEmpty else { return window }
        let overhead = 112 + 24 * 24 + 48 * crops.count
        let available = Swift.max(1, OBCWeatherCodec.producerPolicyMaximumLength - overhead)
        let bytesPerTile = 12 + OBCPrecipitationTileCodec.raw4Length
        let tilesPerFrame = Swift.max(1, available / crops.count / bytesPerTile)
        let tilesPerAxis = Swift.max(1, Int(Double(tilesPerFrame).squareRoot()))
        let cellsPerAxis = tilesPerAxis * OBCPrecipitationTileCodec.tileEdge

        guard window.outputColumns > cellsPerAxis || window.rows > cellsPerAxis else { return window }
        // Scale the *source* axes: the output column count follows from them, so trimming here is
        // what keeps the window's corners on the lattice.
        let columnScale = Double(cellsPerAxis) / Double(Swift.max(1, window.outputColumns))
        let rowScale = Double(cellsPerAxis) / Double(Swift.max(1, window.rows))
        return window.resized(
            sourceColumns: Swift.max(
                1, Int((Double(window.sourceColumns) * columnScale).rounded(.down))),
            rows: Swift.max(1, Int((Double(window.rows) * rowScale).rounded(.down))))
    }

    /// Copy one crop onto `window`: rows 1:1, columns nearest-neighbour onto the uniform pitch.
    private func rainFrame(crop: PrecipitationCrop, window: CommonWindow) -> OBCWeatherRainFrame? {
        let cell = window.cellMicrodegrees
        guard cell > 0 else { return nil }
        let width = window.outputColumns
        let height = window.rows
        guard width > 0, height > 0, width <= Int(UInt16.max), height <= Int(UInt16.max) else {
            return nil
        }
        let columnOffset = Int(floorDivide(window.west - crop.westMicrodegrees, cell))
        let rowOffset = Int(floorDivide(window.south - crop.southMicrodegrees, cell))

        // The source column each output column samples, computed once per frame rather than once per
        // cell: it is the same map for every row, and for every frame of the bundle.
        let sourceColumns = (0..<width).map { output in
            columnOffset + CommonWindow.sourceColumn(
                output: output, sourceColumns: window.sourceColumns, outputColumns: width)
        }

        let edge = OBCPrecipitationTileCodec.tileEdge
        let tileColumns = (width + edge - 1) / edge
        let tileRows = (height + edge - 1) / edge
        var tiles: [[UInt8]] = []
        tiles.reserveCapacity(tileColumns * tileRows)
        var sawNoData = false
        for tileRow in 0..<tileRows {
            for tileColumn in 0..<tileColumns {
                var cells = [UInt8](repeating: OBCPrecipitationTileCodec.noData,
                                    count: OBCPrecipitationTileCodec.tileCells)
                for localRow in 0..<edge {
                    let row = tileRow * edge + localRow
                    guard row < height else { continue }  // north padding stays no-data (§5)
                    let sourceRow = row + rowOffset
                    for localColumn in 0..<edge {
                        let column = tileColumn * edge + localColumn
                        guard column < width else { continue }  // east padding stays no-data
                        let sourceColumn = sourceColumns[column]
                        // A cell the source frame does not reach is no-data, never dry: the window
                        // may extend past what the shards covered, and missing rain must never read
                        // as an absence of rain.
                        guard sourceRow >= 0, sourceRow < crop.height,
                              sourceColumn >= 0, sourceColumn < crop.width
                        else { sawNoData = true; continue }
                        let value = crop.cells[sourceRow * crop.width + sourceColumn]
                        if value == OBCPrecipitationTileCodec.noData { sawNoData = true }
                        cells[localRow * edge + localColumn] = value
                    }
                }
                tiles.append(cells)
            }
        }
        var quality = OBCWeatherQuality(rawValue: crop.quality.rawValue)
        if sawNoData { quality.insert(.partialCoverage) }
        return OBCWeatherRainFrame(
            validAtUnixSeconds: Int64(crop.validAt.timeIntervalSince1970.rounded()),
            width: UInt16(width), height: UInt16(height), cellSizeMetres: crop.cellSizeMetres,
            quality: quality, tiles: tiles)
    }
}
