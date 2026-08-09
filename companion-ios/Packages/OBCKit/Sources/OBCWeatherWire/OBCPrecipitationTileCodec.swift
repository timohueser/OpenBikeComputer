import Foundation

/// Provider-neutral precipitation intensity and canonical raw4/RLE4 tile authority.
///
/// OBCW uses this directly; the future OBCG crop path can reuse the same implementation without
/// copying thresholds or compression rules.
public enum OBCPrecipitationTileCodec {
    public static let tileEdge = 16
    public static let tileCells = tileEdge * tileEdge
    public static let raw4Length = tileCells / 2

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

    public static func encodedLength(_ cells: [UInt8]) throws -> Int {
        guard cells.count == tileCells, cells.allSatisfy(validIntensity) else {
            throw OBCWeatherWireError.malformed
        }
        return min(rleLength(cells), raw4Length)
    }

    /// Encode one tile with deterministic raw4/RLE4 selection.
    public static func encode(_ cells: [UInt8]) throws -> Encoding {
        let length = try encodedLength(cells)
        if length == raw4Length {
            var bytes = Data(count: raw4Length)
            for cell in stride(from: 0, to: tileCells, by: 2) {
                bytes[cell / 2] = cells[cell] | (cells[cell + 1] << 4)
            }
            return Encoding(codec: raw4, bytes: bytes)
        }

        var bytes = Data(); bytes.reserveCapacity(length)
        var cell = 0
        while cell < tileCells {
            var run = 1
            while cell + run < tileCells, run < 16, cells[cell + run] == cells[cell] { run += 1 }
            bytes.append(UInt8((run - 1) << 4) | cells[cell])
            cell += run
        }
        guard bytes.count == length else { throw OBCWeatherWireError.malformed }
        return Encoding(codec: rle4, bytes: bytes)
    }

    /// Decode one canonical tile. Wrong codec choices and non-maximal RLE are rejected.
    public static func decode(codec: UInt8, encoded: Data) throws -> [UInt8] {
        guard !encoded.isEmpty, encoded.count <= raw4Length else {
            throw OBCWeatherWireError.malformed
        }
        if codec == raw4, encoded.count == raw4Length {
            var cells: [UInt8] = []; cells.reserveCapacity(tileCells)
            for byte in encoded {
                let low = byte & 0x0F, high = byte >> 4
                guard validIntensity(low), validIntensity(high) else { throw OBCWeatherWireError.malformed }
                cells.append(low); cells.append(high)
            }
            guard try encodedLength(cells) == raw4Length else { throw OBCWeatherWireError.malformed }
            return cells
        }
        if codec == rle4, encoded.count < raw4Length {
            var cells: [UInt8] = []; cells.reserveCapacity(tileCells)
            var previous: (value: UInt8, run: Int)?
            for byte in encoded {
                let value = byte & 0x0F
                let run = Int(byte >> 4) + 1
                guard validIntensity(value),
                      previous.map({ $0.value != value || $0.run == 16 }) ?? true,
                      cells.count <= tileCells - run else {
                    throw OBCWeatherWireError.malformed
                }
                cells.append(contentsOf: repeatElement(value, count: run))
                previous = (value, run)
            }
            guard cells.count == tileCells else { throw OBCWeatherWireError.malformed }
            return cells
        }
        throw OBCWeatherWireError.malformed
    }

    private static func rleLength(_ cells: [UInt8]) -> Int {
        var runs = 0, cell = 0
        while cell < tileCells {
            var run = 1
            while cell + run < tileCells, run < 16, cells[cell + run] == cells[cell] { run += 1 }
            runs += 1
            cell += run
        }
        return runs
    }
}
