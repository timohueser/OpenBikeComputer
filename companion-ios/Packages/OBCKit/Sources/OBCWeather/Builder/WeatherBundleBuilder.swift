import Foundation
import OBCWeatherWire

public enum WeatherBundleBuildError: Error, Equatable, Sendable {
    /// The hourly section is the one part a bundle cannot be built without. 24 consecutive hours or
    /// nothing — padding the gap would put invented weather on a device that cannot tell.
    case hourlyUnusable
    /// A corridor with no positive extent; nothing to describe.
    case invalidBounds
    /// Even a single frame and the smallest window exceeded the 64 KiB producer policy.
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

/// Merges the two independently selected halves — MET's hourly forecast and whatever precipitation
/// product the corridor earned — into one OBCW object.
///
/// Three rules shape everything here:
///
/// 1. **The halves are independent.** A missing, degraded or expired rain product never discards a
///    valid hourly forecast; a missing hourly forecast is a failed job, because there is no bundle
///    without it.
/// 2. **The re-encode is mechanical.** OBCG crops arrive on the source lattice and are copied onto
///    OBCW's lattice cell for cell. Nothing is resampled, interpolated or smoothed, and a frame
///    whose lattice cannot tile the common window exactly is *dropped and reported* rather than
///    quietly resampled into place.
/// 3. **The output is deterministic.** Same inputs, byte-identical bundle — which is what makes the
///    golden test meaningful and what lets the device's generation/CRC comparison mean something.
public struct WeatherBundleBuilder: Sendable {
    /// How many times the common window may be shrunk before frames start being dropped. Each step
    /// removes an eighth of each axis, so this reaches a single tile long before it runs out.
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

        var bounds = try commonWindow(crops: crops, corridor: corridor)
        bounds = budgeted(bounds, crops: crops)
        var frames: [OBCWeatherRainFrame] = []
        var encoded: Data?

        var attempt = 0
        var usable = crops
        while true {
            let built = rainFrames(crops: usable, window: bounds)
            frames = built.frames
            diagnostics.droppedIncompatibleFrames = built.dropped
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
                // Too big for the 64 KiB producer policy. Trim the *window* first — a slightly
                // shorter corridor still answers the two-hour question at every timestamp, while
                // dropping frames puts holes in the timeline. Only once the window cannot usefully
                // shrink do the furthest-future frames go, and both facts are reported.
                if attempt < Self.maximumShrinkAttempts, let shrunk = bounds.shrunk(towards: request) {
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
        let attributions = ([hourly.attribution] + (selection.map { [$0.attribution] } ?? []))
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

    /// The common `[south, north) x [west, east)` window every frame shares, stated on the coarsest
    /// frame's lattice.
    struct CommonWindow: Equatable {
        var south: Int64
        var west: Int64
        var latitudeStride: Int64
        var longitudeStride: Int64
        var columns: Int
        var rows: Int
        /// The cell the rider sits in, so shrinking keeps them inside the window.
        var anchorColumn: Int
        var anchorRow: Int

        var north: Int64 { south + Int64(rows) * latitudeStride }
        var east: Int64 { west + Int64(columns) * longitudeStride }

        var obcwBounds: OBCWeatherBounds {
            OBCWeatherBounds(
                southLatitudeMicrodegrees: Int32(south), westLongitudeMicrodegrees: Int32(west),
                northLatitudeMicrodegrees: Int32(north), eastLongitudeMicrodegrees: Int32(east),
                gridOriginLatitudeMicrodegrees: Int32(south),
                gridOriginLongitudeMicrodegrees: Int32(west))
        }

        /// Remove an eighth of each axis, keeping the rider's cell inside. Returns `nil` once the
        /// window is a single cell in either direction and cannot usefully shrink again.
        func shrunk(towards request: WeatherRequest) -> CommonWindow? {
            guard columns > 1 || rows > 1 else { return nil }
            return resized(
                columns: Swift.max(1, columns - Swift.max(1, columns / 8)),
                rows: Swift.max(1, rows - Swift.max(1, rows / 8)))
        }

        /// Re-centre the window on the rider at a smaller size, in whole cells of its own lattice.
        func resized(columns newColumns: Int, rows newRows: Int) -> CommonWindow {
            var resized = self
            let keptColumns = Swift.max(1, Swift.min(columns, newColumns))
            let keptRows = Swift.max(1, Swift.min(rows, newRows))
            let west = Swift.min(
                Swift.max(0, anchorColumn - keptColumns / 2), columns - keptColumns)
            let south = Swift.min(Swift.max(0, anchorRow - keptRows / 2), rows - keptRows)
            resized.west += Int64(west) * longitudeStride
            resized.south += Int64(south) * latitudeStride
            resized.columns = keptColumns
            resized.rows = keptRows
            resized.anchorColumn = anchorColumn - west
            resized.anchorRow = anchorRow - south
            return resized
        }
    }

    /// Choose the window every frame is expressed over.
    ///
    /// It is the **coarsest** frame's crop, because a coarse lattice can only be tiled exactly by a
    /// window made of its own cells; deriving the window from a fine frame would make every coarse
    /// frame incompatible and drop the whole model tier. With no frames at all the window is the
    /// corridor itself, so an hourly-only bundle still states the region it describes.
    private func commonWindow(
        crops: [PrecipitationCrop], corridor: WeatherCorridor
    ) throws -> CommonWindow {
        guard let coarsest = crops.max(by: { lhs, rhs in
            let left = UInt64(lhs.latitudeStrideMicrodegrees) * UInt64(lhs.longitudeStrideMicrodegrees)
            let right = UInt64(rhs.latitudeStrideMicrodegrees) * UInt64(rhs.longitudeStrideMicrodegrees)
            if left != right { return left < right }
            return lhs.validAt < rhs.validAt
        }) else {
            let bounds = corridor.bounds
            guard bounds.isWellFormed else { throw WeatherBundleBuildError.invalidBounds }
            return CommonWindow(
                south: bounds.southMicrodegrees, west: bounds.westMicrodegrees,
                latitudeStride: bounds.northMicrodegrees - bounds.southMicrodegrees,
                longitudeStride: bounds.eastMicrodegrees - bounds.westMicrodegrees,
                columns: 1, rows: 1, anchorColumn: 0, anchorRow: 0)
        }
        let latitudeStride = Int64(coarsest.latitudeStrideMicrodegrees)
        let longitudeStride = Int64(coarsest.longitudeStrideMicrodegrees)
        let centre = corridor.bounds
        let anchorColumn = Int(Swift.max(0, Swift.min(
            Int64(coarsest.width) - 1,
            ((centre.westMicrodegrees + centre.eastMicrodegrees) / 2 - coarsest.westMicrodegrees)
                / longitudeStride)))
        let anchorRow = Int(Swift.max(0, Swift.min(
            Int64(coarsest.height) - 1,
            ((centre.southMicrodegrees + centre.northMicrodegrees) / 2 - coarsest.southMicrodegrees)
                / latitudeStride)))
        return CommonWindow(
            south: coarsest.southMicrodegrees, west: coarsest.westMicrodegrees,
            latitudeStride: latitudeStride, longitudeStride: longitudeStride,
            columns: coarsest.width, rows: coarsest.height,
            anchorColumn: anchorColumn, anchorRow: anchorRow)
    }

    /// Trim the window to a size that has a chance of fitting the 64 KiB producer policy, using the
    /// worst case (every tile raw4) and the *finest* frame, which is the binding constraint.
    ///
    /// This is an optimisation of the shrink loop, not a second policy: the loop below is still what
    /// decides, and real RLE4 payloads are far smaller than this estimate, so the first estimate is
    /// usually already generous. Without it, a 400 x 400-cell corridor would encode nine full frames
    /// two dozen times before converging.
    private func budgeted(_ window: CommonWindow, crops: [PrecipitationCrop]) -> CommonWindow {
        guard !crops.isEmpty else { return window }
        let overhead = 112 + 24 * 24 + 48 * crops.count
        let available = Swift.max(1, OBCWeatherCodec.producerPolicyMaximumLength - overhead)
        let bytesPerTile = 12 + OBCPrecipitationTileCodec.raw4Length
        let tilesPerFrame = Swift.max(1, available / crops.count / bytesPerTile)
        let tilesPerAxis = Swift.max(1, Int(Double(tilesPerFrame).squareRoot()))
        let cellsPerAxis = tilesPerAxis * OBCPrecipitationTileCodec.tileEdge

        // How many cells the finest frame would need across this window.
        var finestColumns = window.columns
        var finestRows = window.rows
        for crop in crops {
            let columns = Int((window.east - window.west) / Int64(crop.longitudeStrideMicrodegrees))
            let rows = Int((window.north - window.south) / Int64(crop.latitudeStrideMicrodegrees))
            finestColumns = Swift.max(finestColumns, columns)
            finestRows = Swift.max(finestRows, rows)
        }
        guard finestColumns > cellsPerAxis || finestRows > cellsPerAxis else { return window }
        let columnScale = Double(cellsPerAxis) / Double(Swift.max(1, finestColumns))
        let rowScale = Double(cellsPerAxis) / Double(Swift.max(1, finestRows))
        return window.resized(
            columns: Swift.max(1, Int((Double(window.columns) * columnScale).rounded(.down))),
            rows: Swift.max(1, Int((Double(window.rows) * rowScale).rounded(.down))))
    }

    /// Copy each crop onto `window`, dropping any frame whose lattice cannot tile it exactly.
    private func rainFrames(
        crops: [PrecipitationCrop], window: CommonWindow
    ) -> (frames: [OBCWeatherRainFrame], dropped: Int) {
        var frames: [OBCWeatherRainFrame] = []
        var dropped = 0
        for crop in crops {
            if let frame = rainFrame(crop: crop, window: window) {
                frames.append(frame)
            } else {
                dropped += 1
            }
        }
        return (frames, dropped)
    }

    private func rainFrame(crop: PrecipitationCrop, window: CommonWindow) -> OBCWeatherRainFrame? {
        let latitudeStride = Int64(crop.latitudeStrideMicrodegrees)
        let longitudeStride = Int64(crop.longitudeStrideMicrodegrees)
        guard latitudeStride > 0, longitudeStride > 0 else { return nil }
        // Exact tiling or nothing: the window's edges must fall on this frame's cell boundaries and
        // its span must be a whole number of this frame's cells. Anything else would need
        // resampling, which the epic forbids end to end.
        let southOffset = window.south - crop.southMicrodegrees
        let westOffset = window.west - crop.westMicrodegrees
        guard southOffset % latitudeStride == 0, westOffset % longitudeStride == 0,
              (window.north - window.south) % latitudeStride == 0,
              (window.east - window.west) % longitudeStride == 0
        else { return nil }
        let width = Int((window.east - window.west) / longitudeStride)
        let height = Int((window.north - window.south) / latitudeStride)
        guard width > 0, height > 0, width <= Int(UInt16.max), height <= Int(UInt16.max) else {
            return nil
        }
        let columnOffset = Int(westOffset / longitudeStride)
        let rowOffset = Int(southOffset / latitudeStride)

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
                        let sourceColumn = column + columnOffset
                        // A cell the source frame does not reach is no-data, never dry: the window
                        // may extend past a finer frame's own crop, and missing rain must never
                        // read as an absence of rain.
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
