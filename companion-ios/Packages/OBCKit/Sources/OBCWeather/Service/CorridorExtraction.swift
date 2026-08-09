import Foundation
import OBCWeatherWire

/// The exact set of bytes one corridor needs out of one OBCG frame, and the crop they decode to.
///
/// This is `OBCG_Spec.md` §7 implemented literally: read the header, compute the covering directory
/// pages *arithmetically*, read those pages, read only the non-dry tiles the corridor touches. A
/// conforming consumer must not need any byte outside those ranges, and the request-accounting test
/// pins that it does not — a corridor question must never cost a whole frame download.
enum CorridorExtraction {
    /// The cell window of `frame` that covers `corridor`, clamped to the grid. `nil` when the
    /// corridor and the grid do not overlap at all.
    struct CellWindow: Equatable {
        var columnMinimum: UInt32
        var rowMinimum: UInt32
        var width: Int
        var height: Int
        /// True when the corridor reaches outside this grid, so the crop answers only part of the
        /// question. It becomes OBCW's partial-coverage flag — never a silent smaller map.
        var isClipped: Bool
    }

    static func window(
        geometry: WeatherFrameGeometry, corridor: WeatherBoundingBox
    ) -> CellWindow? {
        let south = Int64(geometry.southMicrodegrees)
        let west = Int64(geometry.westMicrodegrees)
        let latitudeStride = Int64(geometry.cellLatitudeMicrodegrees)
        let longitudeStride = Int64(geometry.cellLongitudeMicrodegrees)
        let bounds = geometry.bounds
        guard corridor.southMicrodegrees < bounds.northMicrodegrees,
              corridor.northMicrodegrees > bounds.southMicrodegrees,
              corridor.westMicrodegrees < bounds.eastMicrodegrees,
              corridor.eastMicrodegrees > bounds.westMicrodegrees
        else { return nil }

        // Floor division on the microdegree lattice: the cell *containing* each corridor edge. The
        // window is then whole source cells, so the crop is a copy of the source lattice rather
        // than a resampling of it.
        let columnMinimumRaw = floorDivide(corridor.westMicrodegrees - west, longitudeStride)
        let columnMaximumRaw = floorDivide(corridor.eastMicrodegrees - west, longitudeStride)
        let rowMinimumRaw = floorDivide(corridor.southMicrodegrees - south, latitudeStride)
        let rowMaximumRaw = floorDivide(corridor.northMicrodegrees - south, latitudeStride)

        let columnMinimum = Swift.max(0, columnMinimumRaw)
        let rowMinimum = Swift.max(0, rowMinimumRaw)
        let columnMaximum = Swift.min(Int64(geometry.width) - 1, columnMaximumRaw)
        let rowMaximum = Swift.min(Int64(geometry.height) - 1, rowMaximumRaw)
        guard columnMinimum <= columnMaximum, rowMinimum <= rowMaximum else { return nil }

        return CellWindow(
            columnMinimum: UInt32(columnMinimum), rowMinimum: UInt32(rowMinimum),
            width: Int(columnMaximum - columnMinimum + 1),
            height: Int(rowMaximum - rowMinimum + 1),
            isClipped: columnMinimumRaw < 0 || rowMinimumRaw < 0
                || columnMaximumRaw > Int64(geometry.width) - 1
                || rowMaximumRaw > Int64(geometry.height) - 1)
    }

    /// Byte ranges, in ascending order, that a reader must fetch for `window` — directory pages
    /// first, then the non-dry tile payloads once the pages have been read.
    static func pageRanges(header: OBCGridHeader, window: CellWindow) throws -> [Range<Int>] {
        var pages: [Int] = []
        for tile in try tileIndexes(header: header, window: window) {
            let page = header.pageOfEntry(tile)
            if !pages.contains(page) { pages.append(page) }
        }
        return try pages.sorted().map { page in
            guard let offset = header.pageOffset(page) else { throw OBCWeatherWireError.malformed }
            return offset..<(offset + header.pageBytes)
        }
    }

    static func tileIndexes(header: OBCGridHeader, window: CellWindow) throws -> [Int] {
        let edge = Int(header.tileEdge)
        guard edge > 0 else { throw OBCWeatherWireError.malformed }
        let columnMinimum = Int(window.columnMinimum) / edge
        let columnMaximum = (Int(window.columnMinimum) + window.width - 1) / edge
        let rowMinimum = Int(window.rowMinimum) / edge
        let rowMaximum = (Int(window.rowMinimum) + window.height - 1) / edge
        var indexes: [Int] = []
        for row in rowMinimum...rowMaximum {
            for column in columnMinimum...columnMaximum {
                guard let index = header.tileIndex(
                    OBCGridTileCoordinate(column: column, row: row))
                else { throw OBCWeatherWireError.malformed }
                indexes.append(index)
            }
        }
        return indexes
    }

    /// Merge byte ranges that touch, so consecutive pages or consecutive tile payloads ride one
    /// HTTP request. OBCG §7 permits coalescing; this only ever merges ranges that are already
    /// adjacent, so it never fetches a byte the corridor did not need.
    static func coalesce(_ ranges: [Range<Int>]) -> [Range<Int>] {
        let sorted = ranges.sorted { $0.lowerBound < $1.lowerBound }
        var merged: [Range<Int>] = []
        for range in sorted {
            if let last = merged.last, range.lowerBound <= last.upperBound {
                merged[merged.count - 1] = last.lowerBound..<Swift.max(last.upperBound, range.upperBound)
            } else {
                merged.append(range)
            }
        }
        return merged
    }

    private static func floorDivide(_ numerator: Int64, _ denominator: Int64) -> Int64 {
        precondition(denominator > 0)
        let quotient = numerator / denominator
        return numerator % denominator < 0 ? quotient - 1 : quotient
    }
}
