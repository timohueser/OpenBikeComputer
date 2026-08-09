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
    static let maximumInputBytes = 8 * 1_024 * 1_024
    static let maximumMessageBytes = 3 * 1_024 * 1_024
    static let maximumGridDimension = 512
    static let maximumGridPointCount = maximumGridDimension * maximumGridDimension
    static let maximumMessageCount = 4
    static let maximumForecastHour = 384

    private static let coordinateToleranceDegrees = 0.000_002

    public init() {}

    /// Decodes cumulative (`startForecastHour == 0`) APCP messages.
    ///
    /// NOMADS currently emits the same cumulative APCP message twice for some
    /// forecast hours. Exact duplicates are collapsed; conflicting duplicates
    /// are rejected. Interval-only messages (for example 6-9 h beside 0-9 h)
    /// are ignored so callers can derive rates by differencing two cumulative
    /// fields and dividing by their forecast-hour delta.
    public func decode(_ data: Data) throws -> [GRIB2PrecipitationGrid] {
        guard data.count <= Self.maximumInputBytes else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "GRIB input exceeds audited bbox limit"
            )
        }
        let bytes = [UInt8](data)
        var offset = 0
        var messageCount = 0
        var cumulative: [GRIB2PrecipitationGrid] = []

        while offset < bytes.count {
            messageCount += 1
            guard messageCount <= Self.maximumMessageCount else {
                throw GRIB2PrecipitationDecoderError.unsupported(
                    reason: "too many GRIB messages for one filtered response"
                )
            }
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
        guard messageLength <= Self.maximumMessageBytes else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "GRIB message exceeds audited bbox limit"
            )
        }
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
        let period = try parseProduct(
            bytes,
            section: sections[4]!,
            referenceTime: referenceTime
        )
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
        guard bytes[base + 11] == 1,
              bytes[base + 19] == 0,
              bytes[base + 20] == 1
        else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only operational forecast reference times"
            )
        }
        let timestamp = GRIB2Timestamp(
            year: Int(try uint16(bytes, base + 12)),
            month: Int(bytes[base + 14]),
            day: Int(bytes[base + 15]),
            hour: Int(bytes[base + 16]),
            minute: Int(bytes[base + 17]),
            second: Int(bytes[base + 18])
        )
        _ = try utcDate(timestamp, context: "reference time")
        return timestamp
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
        guard section.count == 72 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "unexpected grid section length"
            )
        }
        let base = section.lowerBound
        guard bytes[base + 5] == 0,
              bytes[base + 10] == 0,
              bytes[base + 11] == 0
        else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only directly specified grids without optional points"
            )
        }
        guard try uint16(bytes, base + 12) == 0 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only regular latitude/longitude grid template 3.0"
            )
        }
        guard bytes[base + 14] == 6 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only the GFS spherical-earth grid"
            )
        }
        let width = Int(try uint32(bytes, base + 30))
        let height = Int(try uint32(bytes, base + 34))
        let pointCount = Int(try uint32(bytes, base + 6))
        guard width > 0,
              height > 0,
              width <= Self.maximumGridDimension,
              height <= Self.maximumGridDimension,
              pointCount <= Self.maximumGridPointCount,
              width <= pointCount / height,
              width * height == pointCount
        else {
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
              nearlyEqual(longitudeIncrement, 0.25),
              nearlyEqual(latitudeIncrement, 0.25)
        else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "invalid grid coordinates or increments"
            )
        }
        let expectedLastLongitude = normalizeLongitude(
            firstLongitude + Double(width - 1) * longitudeIncrement
        )
        let latitudeDirection = scanningMode == 64 ? 1.0 : -1.0
        let expectedLastLatitude = firstLatitude
            + latitudeDirection * Double(height - 1) * latitudeIncrement
        guard longitudesAreEquivalent(lastLongitude, expectedLastLongitude),
              nearlyEqual(lastLatitude, expectedLastLatitude)
        else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "grid endpoints contradict dimensions, increments, or scanning mode"
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
        _ bytes: [UInt8],
        section: Range<Int>,
        referenceTime: GRIB2Timestamp
    ) throws -> (startHour: Int, endHour: Int) {
        guard section.count == 58 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "unexpected product section length"
            )
        }
        let base = section.lowerBound
        guard try uint16(bytes, base + 5) == 0 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "product-specific coordinate values"
            )
        }
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
        guard bytes[base + 11] == 2,
              bytes[base + 14] == 0,
              try uint16(bytes, base + 15) == 0
        else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only uncut forecast products"
            )
        }
        guard bytes[base + 17] == 1, bytes[base + 48] == 1 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only hour-based forecast periods"
            )
        }
        guard bytes[base + 22] == 1,
              bytes[base + 23] == 0,
              try uint32(bytes, base + 24) == 0,
              bytes[base + 28] == 255
        else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only a single surface-level field"
            )
        }
        guard bytes[base + 41] == 1,
              try uint32(bytes, base + 42) == 0,
              bytes[base + 46] == 1,
              bytes[base + 47] == 2,
              bytes[base + 53] == 255,
              try uint32(bytes, base + 54) == 0
        else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "only one accumulation time range"
            )
        }
        let start = Int(try uint32(bytes, base + 18))
        let duration = Int(try uint32(bytes, base + 49))
        guard duration > 0,
              start <= Self.maximumForecastHour,
              duration <= Self.maximumForecastHour - start
        else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "forecast period exceeds the GFS contract"
            )
        }
        let end = start + duration
        let endTimestamp = GRIB2Timestamp(
            year: Int(try uint16(bytes, base + 34)),
            month: Int(bytes[base + 36]),
            day: Int(bytes[base + 37]),
            hour: Int(bytes[base + 38]),
            minute: Int(bytes[base + 39]),
            second: Int(bytes[base + 40])
        )
        let referenceDate = try utcDate(referenceTime, context: "reference time")
        let endDate = try utcDate(endTimestamp, context: "end of accumulation")
        guard let expectedEndDate = utcCalendar.date(
            byAdding: .hour,
            value: end,
            to: referenceDate
        ), endDate == expectedEndDate else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "accumulation end timestamp contradicts its forecast range"
            )
        }
        return (start, end)
    }

    private func parseValues(
        _ bytes: [UInt8],
        representation: Range<Int>,
        bitmap: Range<Int>,
        data: Range<Int>,
        gridPointCount: Int
    ) throws -> [Double?] {
        guard representation.count == 21 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "unexpected data representation section length"
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
        guard bytes[rep + 20] == 0, bitsPerValue <= 64 else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "unsupported packed-value representation"
            )
        }
        let binaryFactor = pow(2, Double(binaryScale))
        let decimalFactor = pow(10, Double(-decimalScale))
        let scale = binaryFactor * decimalFactor
        let scaledReference = reference * decimalFactor
        guard binaryFactor.isFinite,
              decimalFactor.isFinite,
              scale.isFinite,
              scaledReference.isFinite
        else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "non-finite packing scale"
            )
        }

        guard bitmap.count >= 6 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "short bitmap section")
        }
        let bitmapBase = bitmap.lowerBound
        let bitmapIndicator = bytes[bitmapBase + 5]
        let packedCount: Int
        switch bitmapIndicator {
        case 255:
            guard bitmap.count == 6, declaredValueCount == gridPointCount else {
                throw GRIB2PrecipitationDecoderError.malformed(
                    reason: "no-bitmap value count mismatch"
                )
            }
            packedCount = gridPointCount
        case 0:
            let bitmapByteCount = (gridPointCount + 7) / 8
            guard bitmap.count == 6 + bitmapByteCount else {
                throw GRIB2PrecipitationDecoderError.malformed(
                    reason: "bitmap length does not match grid"
                )
            }
            guard paddingBitsAreZero(
                bytes,
                start: bitmapBase + 6,
                usedBitCount: gridPointCount,
                end: bitmap.upperBound
            ) else {
                throw GRIB2PrecipitationDecoderError.malformed(
                    reason: "non-zero bitmap padding"
                )
            }
            packedCount = (0 ..< gridPointCount).reduce(into: 0) { count, index in
                if bitmapValueIsPresent(bytes, base: bitmapBase, index: index) {
                    count += 1
                }
            }
            guard declaredValueCount == packedCount else {
                throw GRIB2PrecipitationDecoderError.malformed(
                    reason: "bitmap population and packed value count disagree"
                )
            }
        default:
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "predefined bitmap \(bitmapIndicator)"
            )
        }

        guard data.count >= 5 else {
            throw GRIB2PrecipitationDecoderError.malformed(reason: "short data section")
        }
        guard packedCount <= Self.maximumGridPointCount,
              bitsPerValue == 0 || packedCount <= Int.max / bitsPerValue
        else {
            throw GRIB2PrecipitationDecoderError.unsupported(
                reason: "packed field exceeds audited allocation limits"
            )
        }
        let packedBitCount = packedCount * bitsPerValue
        let packedByteCount = (packedBitCount + 7) / 8
        guard data.count == 5 + packedByteCount else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "packed payload length does not match value count"
            )
        }
        guard paddingBitsAreZero(
            bytes,
            start: data.lowerBound + 5,
            usedBitCount: packedBitCount,
            end: data.upperBound
        ) else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "non-zero packed-value padding"
            )
        }
        var reader = BitReader(
            bytes: bytes,
            start: data.lowerBound + 5,
            end: data.upperBound
        )
        var result: [Double?] = []
        result.reserveCapacity(gridPointCount)
        for index in 0 ..< gridPointCount {
            let present = bitmapIndicator == 255
                || bitmapValueIsPresent(bytes, base: bitmapBase, index: index)
            guard present else {
                result.append(nil)
                continue
            }
            let packed = try reader.read(bitCount: bitsPerValue)
            let value = scaledReference + Double(packed) * scale
            guard value.isFinite, value >= 0 else {
                throw GRIB2PrecipitationDecoderError.malformed(
                    reason: "non-finite or negative precipitation value"
                )
            }
            result.append(value)
        }
        return result
    }

    private struct BitReader {
        let bytes: [UInt8]
        let start: Int
        let end: Int
        var bitOffset = 0

        mutating func read(bitCount: Int) throws -> UInt64 {
            let availableByteCount = end - start
            guard bitCount >= 0,
                  bitCount <= 64,
                  availableByteCount >= 0,
                  availableByteCount <= Int.max / 8,
                  bitOffset <= availableByteCount * 8 - bitCount
            else {
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

    private var utcCalendar: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        return calendar
    }

    private func utcDate(
        _ timestamp: GRIB2Timestamp,
        context: String
    ) throws -> Date {
        var components = DateComponents()
        components.calendar = utcCalendar
        components.timeZone = utcCalendar.timeZone
        components.year = timestamp.year
        components.month = timestamp.month
        components.day = timestamp.day
        components.hour = timestamp.hour
        components.minute = timestamp.minute
        components.second = timestamp.second
        guard let date = utcCalendar.date(from: components) else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "invalid \(context)"
            )
        }
        let fields = utcCalendar.dateComponents(
            [.year, .month, .day, .hour, .minute, .second],
            from: date
        )
        guard fields.year == timestamp.year,
              fields.month == timestamp.month,
              fields.day == timestamp.day,
              fields.hour == timestamp.hour,
              fields.minute == timestamp.minute,
              fields.second == timestamp.second
        else {
            throw GRIB2PrecipitationDecoderError.malformed(
                reason: "invalid \(context)"
            )
        }
        return date
    }

    private func normalizeLongitude(_ degrees: Double) -> Double {
        var normalized = degrees.truncatingRemainder(dividingBy: 360)
        if normalized >= 180 { normalized -= 360 }
        if normalized < -180 { normalized += 360 }
        return normalized
    }

    private func nearlyEqual(_ lhs: Double, _ rhs: Double) -> Bool {
        abs(lhs - rhs) <= Self.coordinateToleranceDegrees
    }

    private func longitudesAreEquivalent(_ lhs: Double, _ rhs: Double) -> Bool {
        nearlyEqual(normalizeLongitude(lhs - rhs), 0)
    }

    private func bitmapValueIsPresent(
        _ bytes: [UInt8],
        base: Int,
        index: Int
    ) -> Bool {
        let byte = bytes[base + 6 + index / 8]
        return byte & (1 << (7 - index % 8)) != 0
    }

    private func paddingBitsAreZero(
        _ bytes: [UInt8],
        start: Int,
        usedBitCount: Int,
        end: Int
    ) -> Bool {
        let availableBitCount = (end - start) * 8
        guard usedBitCount <= availableBitCount else { return false }
        for index in usedBitCount ..< availableBitCount {
            let byte = bytes[start + index / 8]
            if byte & (1 << (7 - index % 8)) != 0 { return false }
        }
        return true
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
