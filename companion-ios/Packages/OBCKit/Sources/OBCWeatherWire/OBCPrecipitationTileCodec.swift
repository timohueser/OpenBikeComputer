import Foundation

/// Provider-neutral precipitation intensity and canonical raw4/RLE4 tile authority.
///
/// OBCW always uses the fixed 16 x 16 tile; OBCG picks a power-of-two tile edge per generation, so
/// the codec is generalized over the decoded cell count and the 256-cell entry points are exact
/// wrappers over the generalized ones. Neither container owns a second set of thresholds or
/// compression rules.
public enum OBCPrecipitationTileCodec {
    public static let tileEdge = 16
    public static let tileCells = tileEdge * tileEdge
    public static let raw4Length = tileCells / 2
    /// Largest generalized cell count: a 256 x 256-cell OBCG tile. Its raw4 payload is 32,768
    /// bytes, so every canonical encoded length still fits `uint16`.
    public static let maximumCells = 256 * 256

    public static let raw4: UInt8 = 0
    public static let rle4: UInt8 = 1

    public static let dry: UInt8 = 0
    public static let maximumIntensity: UInt8 = 12
    public static let noData: UInt8 = 15

    public struct Encoding: Equatable, Sendable {
        public let codec: UInt8
        public let bytes: Data
    }

    /// Quantize a finite, non-negative instantaneous precipitation rate in mm/h.
    public static func quantize(rateMillimetresPerHour rate: Double) -> UInt8 {
        if !rate.isFinite || rate < 0 { return noData }
        if rate == 0 { return dry }
        if rate < 0.10 { return 1 }
        if rate < 0.25 { return 2 }
        if rate < 0.50 { return 3 }
        if rate < 1.00 { return 4 }
        if rate < 2.00 { return 5 }
        if rate < 4.00 { return 6 }
        if rate < 6.00 { return 7 }
        if rate < 10.00 { return 8 }
        if rate < 16.00 { return 9 }
        if rate < 25.00 { return 10 }
        if rate < 50.00 { return 11 }
        return maximumIntensity
    }

    public static func validIntensity(_ value: UInt8) -> Bool {
        value <= maximumIntensity || value == noData
    }

    /// True when `count` is a legal generalized cell count: nonzero, even (raw4 packs two cells
    /// per byte) and no larger than `maximumCells`.
    public static func validCellCount(_ count: Int) -> Bool {
        count > 0 && count.isMultiple(of: 2) && count <= maximumCells
    }

    /// Deterministic encoded length of one cell block after validating every intensity code.
    /// OBCW always passes 256 cells; OBCG passes `tileEdge * tileEdge` for its own tile.
    public static func encodedCellsLength(_ cells: [UInt8]) throws -> Int {
        guard validCellCount(cells.count), cells.allSatisfy(validIntensity) else {
            throw OBCWeatherWireError.malformed
        }
        return min(rleLength(cells), cells.count / 2)
    }

    /// Deterministic encoded length of one 16 x 16 tile.
    public static func encodedLength(_ cells: [UInt8]) throws -> Int {
        guard cells.count == tileCells else { throw OBCWeatherWireError.malformed }
        return try encodedCellsLength(cells)
    }

    /// Encode one cell block with deterministic raw4/RLE4 selection; ties use raw4.
    public static func encodeCells(_ cells: [UInt8]) throws -> Encoding {
        let length = try encodedCellsLength(cells)
        let rawLength = cells.count / 2
        if length == rawLength {
            var bytes = Data(count: rawLength)
            for cell in stride(from: 0, to: cells.count, by: 2) {
                bytes[cell / 2] = cells[cell] | (cells[cell + 1] << 4)
            }
            return Encoding(codec: raw4, bytes: bytes)
        }

        var bytes = Data(); bytes.reserveCapacity(length)
        var cell = 0
        while cell < cells.count {
            var run = 1
            while cell + run < cells.count, run < 16, cells[cell + run] == cells[cell] { run += 1 }
            bytes.append(UInt8((run - 1) << 4) | cells[cell])
            cell += run
        }
        guard bytes.count == length else { throw OBCWeatherWireError.malformed }
        return Encoding(codec: rle4, bytes: bytes)
    }

    /// Encode one 16 x 16 tile with deterministic raw4/RLE4 selection.
    public static func encode(_ cells: [UInt8]) throws -> Encoding {
        guard cells.count == tileCells else { throw OBCWeatherWireError.malformed }
        return try encodeCells(cells)
    }

    /// Validate one canonical cell block without expanding it. Wrong codec choices, reserved
    /// intensities, non-maximal RLE and wrong cell sums are all rejected.
    public static func validateCells(codec: UInt8, encoded: Data, cellCount: Int) throws {
        guard validCellCount(cellCount) else { throw OBCWeatherWireError.malformed }
        let rawLength = cellCount / 2
        guard !encoded.isEmpty, encoded.count <= rawLength else { throw OBCWeatherWireError.malformed }
        if codec == raw4, encoded.count == rawLength {
            for byte in encoded {
                guard validIntensity(byte & 0x0F), validIntensity(byte >> 4) else {
                    throw OBCWeatherWireError.malformed
                }
            }
            // raw4 is canonical only when the maximal-run RLE4 encoding is no smaller.
            guard raw4CanonicalRLELength(encoded, cellCount: cellCount) >= rawLength else {
                throw OBCWeatherWireError.malformed
            }
            return
        }
        if codec == rle4, encoded.count < rawLength {
            var count = 0
            var previous: (value: UInt8, run: Int)?
            for byte in encoded {
                let value = byte & 0x0F
                let run = Int(byte >> 4) + 1
                guard validIntensity(value),
                      previous.map({ $0.value != value || $0.run == 16 }) ?? true,
                      count <= cellCount - run else {
                    throw OBCWeatherWireError.malformed
                }
                count += run
                previous = (value, run)
            }
            guard count == cellCount else { throw OBCWeatherWireError.malformed }
            return
        }
        throw OBCWeatherWireError.malformed
    }

    /// Decode one canonical cell block of `cellCount` cells.
    public static func decodeCells(codec: UInt8, encoded: Data, cellCount: Int) throws -> [UInt8] {
        try validateCells(codec: codec, encoded: encoded, cellCount: cellCount)
        var cells: [UInt8] = []; cells.reserveCapacity(cellCount)
        if codec == raw4 {
            for byte in encoded { cells.append(byte & 0x0F); cells.append(byte >> 4) }
        } else {
            for byte in encoded {
                cells.append(contentsOf: repeatElement(byte & 0x0F, count: Int(byte >> 4) + 1))
            }
        }
        guard cells.count == cellCount else { throw OBCWeatherWireError.malformed }
        return cells
    }

    /// Decode one canonical 16 x 16 tile. Wrong codec choices and non-maximal RLE are rejected.
    public static func decode(codec: UInt8, encoded: Data) throws -> [UInt8] {
        try decodeCells(codec: codec, encoded: encoded, cellCount: tileCells)
    }

    /// Count the maximal, 16-cell-capped runs a valid raw4 payload represents, without expanding
    /// it. The result is the canonical RLE4 byte length of the same cells.
    private static func raw4CanonicalRLELength(_ encoded: Data, cellCount: Int) -> Int {
        let bytes = [UInt8](encoded)
        var runs = 0, runLength = 0
        var previous: UInt8?
        for cell in 0..<min(cellCount, bytes.count * 2) {
            let byte = bytes[cell / 2]
            let value = cell.isMultiple(of: 2) ? byte & 0x0F : byte >> 4
            if previous == value, runLength < 16 {
                runLength += 1
            } else {
                runs += 1
                previous = value
                runLength = 1
            }
        }
        return runs
    }

    private static func rleLength(_ cells: [UInt8]) -> Int {
        var runs = 0, cell = 0
        while cell < cells.count {
            var run = 1
            while cell + run < cells.count, run < 16, cells[cell + run] == cells[cell] { run += 1 }
            runs += 1
            cell += run
        }
        return runs
    }
}
