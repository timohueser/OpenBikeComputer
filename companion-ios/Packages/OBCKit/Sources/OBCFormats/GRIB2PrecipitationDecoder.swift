import Foundation

/// The timestamp carried by GRIB2 section 1. It stays calendar-shaped rather
/// than depending on a local time zone; GRIB reference times are always UTC.
public struct GRIB2Timestamp: Equatable, Sendable {
    public let year: Int
    public let month: Int
    public let day: Int
    public let hour: Int
    public let minute: Int
    public let second: Int
}

/// A cumulative NOAA GFS `APCP` field decoded without changing its source grid.
///
/// `valuesMM` is in GRIB scanning order. Missing bitmap cells remain `nil`;
/// callers must not interpolate them or present the 0.25-degree model as a
/// finer spatial forecast.
public struct GRIB2PrecipitationGrid: Equatable, Sendable {
    public let referenceTime: GRIB2Timestamp
    public let startForecastHour: Int
    public let endForecastHour: Int
    public let width: Int
    public let height: Int
    public let latitudeOfFirstPointDegrees: Double
    public let longitudeOfFirstPointDegrees: Double
    public let latitudeOfLastPointDegrees: Double
    public let longitudeOfLastPointDegrees: Double
    public let longitudeIncrementDegrees: Double
    public let latitudeIncrementDegrees: Double
    public let scanningMode: UInt8
    public let valuesMM: [Double?]
}

public enum GRIB2PrecipitationDecoderError: Error, Equatable, Sendable {
    case malformed(reason: String)
    case unsupported(reason: String)
    case conflictingCumulativeField(endForecastHour: Int)
}

/// Audited subset decoder for bbox-filtered NOAA GFS 0.25-degree precipitation.
///
/// The NOMADS filter currently returns regular latitude/longitude (grid
/// template 3.0), cumulative total precipitation (product template 4.8), and
/// simple packing (data template 5.0). Keeping that supported surface narrow is
/// deliberate: an upstream template change fails closed instead of silently
/// producing plausible-looking rain values.
public struct GRIB2PrecipitationDecoder: Sendable {
    public init() {}

    /// Decodes cumulative (`startForecastHour == 0`) APCP messages.
    ///
    /// NOMADS currently emits the same cumulative APCP message twice for some
    /// forecast hours. Exact duplicates are collapsed; conflicting duplicates
    /// are rejected. Interval-only messages (for example 6-9 h beside 0-9 h)
    /// are ignored so callers can derive rates by differencing two cumulative
    /// fields and dividing by their forecast-hour delta.
    public func decode(_ data: Data) throws -> [GRIB2PrecipitationGrid] {
        let bytes = [UInt8](data)
        var offset = 0
        var cumulative: [GRIB2PrecipitationGrid] = []

        while offset < bytes.count {
            let parsed = try parseMessage(bytes, at: offset)
            offset = parsed.nextOffset
            guard parsed.grid.startForecastHour == 0 else { continue }

            if let existing = cumulative.first(where: {
                $0.referenceTime == parsed.grid.referenceTime
                    && $0.endForecastHour == parsed.grid.endForecastHour
            }) {
                guard existing == parsed.grid else {
                    throw GRIB2PrecipitationDecoderError.conflictingCumulativeField(
                        endForecastHour: parsed.grid.endForecastHour
                    )
                }
                continue
            }
            cumulative.append(parsed.grid)
        }

        guard !cumulative.isEmpty else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "no cumulative surface APCP message"
            )
        }
        return cumulative.sorted { $0.endForecastHour < $1.endForecastHour }
    }

    private func parseMessage(
        _ bytes: [UInt8], at messageOffset: Int
    ) throws -> (grid: GRIB2PrecipitationGrid, nextOffset: Int) {
        try require(bytes, messageOffset, 16, "GRIB2 indicator")
        guard Array(bytes[messageOffset ..< messageOffset + 4]) == Array("GRIB".utf8) else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "missing GRIB magic")
        }
        guard bytes[messageOffset + 7] == 2 else {
            throw GRIB2PrecipitationDecoderError.unsupported(reason: "only GRIB edition 2")
        }
        guard bytes[messageOffset + 6] == 0 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only meteorological discipline 0"
            )
        }

        let length64 = try uint64(bytes, messageOffset + 8)
        guard length64 <= UInt64(Int.max), length64 >= 20 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "invalid message length")
        }
        let messageLength = Int(length64)
        guard messageLength <= bytes.count - messageOffset else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "truncated GRIB2 message")
        }
        let messageEnd = messageOffset + messageLength
        guard Array(bytes[messageEnd - 4 ..< messageEnd]) == Array("7777".utf8) else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "missing GRIB2 terminator")
        }

        var sections: [UInt8: Range<Int>] = [:]
        var sectionOffset = messageOffset + 16
        while sectionOffset < messageEnd - 4 {
            try require(bytes, sectionOffset, 5, "section header")
            let sectionLength = Int(try uint32(bytes, sectionOffset))
            guard sectionLength >= 5, sectionOffset + sectionLength <= messageEnd - 4 else {
                throw GRIB2PrecipitationDecoderError.malformed(reason: "invalid section length")
            }
            let number = bytes[sectionOffset + 4]
            guard sections[number] == nil else {
                throw GRIB2PrecipitationDecoderError.malformed(
                    reason: "duplicate section \(number)"
                )
            }
            sections[number] = sectionOffset ..< sectionOffset + sectionLength
            sectionOffset += sectionLength
        }
        guard sectionOffset == messageEnd - 4 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "sections do not fill message")
        }
        for required: UInt8 in [1, 3, 4, 5, 6, 7] where sections[required] == nil {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "missing section \(required)"
            )
        }

        let referenceTime = try parseReferenceTime(bytes, section: sections[1]!)
        let geometry = try parseGeometry(bytes, section: sections[3]!)
        let period = try parseProduct(bytes, section: sections[4]!)
        let values = try parseValues(
            bytes,
            representation: sections[5]!,
            bitmap: sections[6]!,
            data: sections[7]!,
            gridPointCount: geometry.width * geometry.height
        )

        return (
            GRIB2PrecipitationGrid(
                referenceTime: referenceTime,
                startForecastHour: period.startHour,
                endForecastHour: period.endHour,
                width: geometry.width,
                height: geometry.height,
                latitudeOfFirstPointDegrees: geometry.firstLatitude,
                longitudeOfFirstPointDegrees: geometry.firstLongitude,
                latitudeOfLastPointDegrees: geometry.lastLatitude,
                longitudeOfLastPointDegrees: geometry.lastLongitude,
                longitudeIncrementDegrees: geometry.longitudeIncrement,
                latitudeIncrementDegrees: geometry.latitudeIncrement,
                scanningMode: geometry.scanningMode,
                valuesMM: values
            ),
            messageEnd
        )
    }

    private func parseReferenceTime(
        _ bytes: [UInt8], section: Range<Int>
    ) throws -> GRIB2Timestamp {
        guard section.count == 21 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "unexpected identification section"
            )
        }
        let base = section.lowerBound
        return GRIB2Timestamp(
            year: Int(try uint16(bytes, base + 12)),
            month: Int(bytes[base + 14]),
            day: Int(bytes[base + 15]),
            hour: Int(bytes[base + 16]),
            minute: Int(bytes[base + 17]),
            second: Int(bytes[base + 18])
        )
    }

    private struct Geometry {
        let width: Int
        let height: Int
        let firstLatitude: Double
        let firstLongitude: Double
        let lastLatitude: Double
        let lastLongitude: Double
        let longitudeIncrement: Double
        let latitudeIncrement: Double
        let scanningMode: UInt8
    }

    private func parseGeometry(
        _ bytes: [UInt8], section: Range<Int>
    ) throws -> Geometry {
        guard section.count >= 72 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "short grid section")
        }
        let base = section.lowerBound
        guard try uint16(bytes, base + 12) == 0 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only regular latitude/longitude grid template 3.0"
            )
        }
        let width = Int(try uint32(bytes, base + 30))
        let height = Int(try uint32(bytes, base + 34))
        let pointCount = Int(try uint32(bytes, base + 6))
        guard width > 0, height > 0, width <= Int.max / height, width * height == pointCount else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "inconsistent grid dimensions")
        }
        let basicAngle = try uint32(bytes, base + 38)
        let subdivisions = try uint32(bytes, base + 42)
        guard basicAngle == 0, subdivisions == UInt32.max else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "non-default angular units"
            )
        }
        let scanningMode = bytes[base + 71]
        guard scanningMode == 0 || scanningMode == 64 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "scanning mode \(scanningMode)"
            )
        }
        let firstLatitude = Double(try signedMagnitude32(bytes, base + 46)) / 1_000_000
        let firstLongitude = normalizeLongitude(
            Double(try signedMagnitude32(bytes, base + 50)) / 1_000_000
        )
        let lastLatitude = Double(try signedMagnitude32(bytes, base + 55)) / 1_000_000
        let lastLongitude = normalizeLongitude(
            Double(try signedMagnitude32(bytes, base + 59)) / 1_000_000
        )
        let longitudeIncrement = Double(try uint32(bytes, base + 63)) / 1_000_000
        let latitudeIncrement = Double(try uint32(bytes, base + 67)) / 1_000_000
        guard (-90.0 ... 90.0).contains(firstLatitude),
              (-90.0 ... 90.0).contains(lastLatitude),
              (-180.0 ... 180.0).contains(firstLongitude),
              (-180.0 ... 180.0).contains(lastLongitude),
              longitudeIncrement > 0,
              longitudeIncrement <= 360,
              latitudeIncrement > 0,
              latitudeIncrement <= 180
        else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "invalid grid coordinates or increments"
            )
        }
        return Geometry(
            width: width,
            height: height,
            firstLatitude: firstLatitude,
            firstLongitude: firstLongitude,
            lastLatitude: lastLatitude,
            lastLongitude: lastLongitude,
            longitudeIncrement: longitudeIncrement,
            latitudeIncrement: latitudeIncrement,
            scanningMode: scanningMode
        )
    }

    private func parseProduct(
        _ bytes: [UInt8], section: Range<Int>
    ) throws -> (startHour: Int, endHour: Int) {
        guard section.count >= 58 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "short product section")
        }
        let base = section.lowerBound
        guard try uint16(bytes, base + 7) == 8 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only product definition template 4.8"
            )
        }
        guard bytes[base + 9] == 1, bytes[base + 10] == 8 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only total precipitation (category 1, parameter 8)"
            )
        }
        guard bytes[base + 17] == 1, bytes[base + 48] == 1 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only hour-based forecast periods"
            )
        }
        guard bytes[base + 22] == 1 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only surface precipitation"
            )
        }
        guard bytes[base + 41] == 1, bytes[base + 46] == 1 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only one accumulation time range"
            )
        }
        let start = Int(try uint32(bytes, base + 18))
        let duration = Int(try uint32(bytes, base + 49))
        guard start <= Int.max - duration else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "forecast hour overflow")
        }
        return (start, start + duration)
    }

    private func parseValues(
        _ bytes: [UInt8],
        representation: Range<Int>,
        bitmap: Range<Int>,
        data: Range<Int>,
        gridPointCount: Int
    ) throws -> [Double?] {
        guard representation.count >= 21 else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "short data representation section"
            )
        }
        let rep = representation.lowerBound
        guard try uint16(bytes, rep + 9) == 0 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only simple packing template 5.0"
            )
        }
        let declaredValueCount = Int(try uint32(bytes, rep + 5))
        let referenceBits = try uint32(bytes, rep + 11)
        let reference = Double(Float(bitPattern: referenceBits))
        guard reference.isFinite else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "non-finite packing reference"
            )
        }
        let binaryScale = try signedMagnitude16(bytes, rep + 15)
        let decimalScale = try signedMagnitude16(bytes, rep + 17)
        let bitsPerValue = Int(bytes[rep + 19])
        let scale = pow(2, Double(binaryScale)) * pow(10, Double(-decimalScale))
        let scaledReference = reference * pow(10, Double(-decimalScale))

        guard bitmap.count >= 6 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "short bitmap section")
        }
        let bitmapBase = bitmap.lowerBound
        let bitmapIndicator = bytes[bitmapBase + 5]
        let presence: [Bool]
        switch bitmapIndicator {
        case 255:
            presence = Array(repeating: true, count: gridPointCount)
        case 0:
            let availableBits = (bitmap.count - 6) * 8
            guard availableBits >= gridPointCount else {
                throw GRIB2PrecipitationDecoderError.malformed(reason: "short bitmap")
            }
            presence = (0 ..< gridPointCount).map { index in
                let byte = bytes[bitmapBase + 6 + index / 8]
                return byte & (1 << (7 - index % 8)) != 0
            }
        default:
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "predefined bitmap \(bitmapIndicator)"
            )
        }

        let packedCount = presence.reduce(into: 0) { count, present in
            if present { count += 1 }
        }
        guard declaredValueCount == gridPointCount || declaredValueCount == packedCount else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "inconsistent packed value count"
            )
        }
        guard data.count >= 5 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "short data section")
        }
        var reader = BitReader(
            bytes: bytes,
            start: data.lowerBound + 5,
            end: data.upperBound
        )
        var result: [Double?] = []
        result.reserveCapacity(gridPointCount)
        for present in presence {
            guard present else {
                result.append(nil)
                continue
            }
            let packed = try reader.read(bitCount: bitsPerValue)
            result.append(scaledReference + Double(packed) * scale)
        }
        return result
    }

    private struct BitReader {
        let bytes: [UInt8]
        let start: Int
        let end: Int
        var bitOffset = 0

        mutating func read(bitCount: Int) throws -> UInt64 {
            guard bitCount <= 64, bitOffset + bitCount <= (end - start) * 8 else {
                throw GRIB2PrecipitationDecoderError.malformed(reason: "truncated packed values")
            }
            var value: UInt64 = 0
            for _ in 0 ..< bitCount {
                let absolute = bitOffset
                let byte = bytes[start + absolute / 8]
                value = (value << 1) | UInt64((byte >> (7 - absolute % 8)) & 1)
                bitOffset += 1
            }
            return value
        }
    }

    private func normalizeLongitude(_ degrees: Double) -> Double {
        degrees > 180 ? degrees - 360 : degrees
    }

    private func require(
        _ bytes: [UInt8], _ offset: Int, _ count: Int, _ context: String
    ) throws {
        guard offset >= 0, count >= 0, offset <= bytes.count - count else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "truncated \(context)")
        }
    }

    private func uint16(_ bytes: [UInt8], _ offset: Int) throws -> UInt16 {
        try require(bytes, offset, 2, "uint16")
        return UInt16(bytes[offset]) << 8 | UInt16(bytes[offset + 1])
    }

    private func uint32(_ bytes: [UInt8], _ offset: Int) throws -> UInt32 {
        try require(bytes, offset, 4, "uint32")
        return UInt32(bytes[offset]) << 24
            | UInt32(bytes[offset + 1]) << 16
            | UInt32(bytes[offset + 2]) << 8
            | UInt32(bytes[offset + 3])
    }

    private func uint64(_ bytes: [UInt8], _ offset: Int) throws -> UInt64 {
        try require(bytes, offset, 8, "uint64")
        var value: UInt64 = 0
        for byte in bytes[offset ..< offset + 8] {
            value = value << 8 | UInt64(byte)
        }
        return value
    }

    private func signedMagnitude16(_ bytes: [UInt8], _ offset: Int) throws -> Int {
        let raw = try uint16(bytes, offset)
        let magnitude = Int(raw & 0x7fff)
        return raw & 0x8000 == 0 ? magnitude : -magnitude
    }

    private func signedMagnitude32(_ bytes: [UInt8], _ offset: Int) throws -> Int {
        let raw = try uint32(bytes, offset)
        let magnitude = Int(raw & 0x7fff_ffff)
        return raw & 0x8000_0000 == 0 ? magnitude : -magnitude
    }
}
