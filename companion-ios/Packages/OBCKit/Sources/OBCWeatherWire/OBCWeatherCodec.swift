import Foundation

/// OBCW/OBCG v1 errors. The public boundary intentionally does not expose parser offsets or
/// provider concepts; malformed input is untrusted bytes, while the producer-policy error is
/// actionable.
public enum OBCWeatherWireError: Error, Equatable, Sendable {
    case malformed
    case crcMismatch
    case producerPolicyExceeded
}

public struct OBCWeatherBounds: Equatable, Sendable {
    public var southLatitudeMicrodegrees: Int32
    public var westLongitudeMicrodegrees: Int32
    public var northLatitudeMicrodegrees: Int32
    public var eastLongitudeMicrodegrees: Int32
    public var gridOriginLatitudeMicrodegrees: Int32
    public var gridOriginLongitudeMicrodegrees: Int32

    public init(
        southLatitudeMicrodegrees: Int32, westLongitudeMicrodegrees: Int32,
        northLatitudeMicrodegrees: Int32, eastLongitudeMicrodegrees: Int32,
        gridOriginLatitudeMicrodegrees: Int32, gridOriginLongitudeMicrodegrees: Int32
    ) {
        self.southLatitudeMicrodegrees = southLatitudeMicrodegrees
        self.westLongitudeMicrodegrees = westLongitudeMicrodegrees
        self.northLatitudeMicrodegrees = northLatitudeMicrodegrees
        self.eastLongitudeMicrodegrees = eastLongitudeMicrodegrees
        self.gridOriginLatitudeMicrodegrees = gridOriginLatitudeMicrodegrees
        self.gridOriginLongitudeMicrodegrees = gridOriginLongitudeMicrodegrees
    }
}

public enum OBCWeatherCondition: UInt8, CaseIterable, Sendable {
    case clear = 0
    case mostlyClear = 1
    case partlyCloudy = 2
    case overcast = 3
    case fog = 4
    case drizzle = 5
    case rain = 6
    case sleet = 7
    case snow = 8
    case showers = 9
    case thunderstorm = 10
    case hail = 11
    case wind = 12
    case unavailable = 255
}

/// Fixed-width following-hour wire values. Record `i` begins at `validFrom + i*3600`; amount and
/// probability describe `[validAt, validAt+3600)`. Unavailable sentinels remain explicit here;
/// WX4's semantic domain layer owns any optional-value translation.
public struct OBCWeatherHourlyRecord: Equatable, Sendable {
    public var validTimeOffsetSeconds: UInt32
    public var temperatureDeciCelsius: Int16
    public var precipitationTenthMillimetres: UInt16
    public var precipitationProbabilityPercent: UInt8
    public var condition: OBCWeatherCondition
    public var windFromDegrees: UInt16
    public var windSpeedDeciMetresPerSecond: UInt16
    public var windGustDeciMetresPerSecond: UInt16

    public init(
        validTimeOffsetSeconds: UInt32, temperatureDeciCelsius: Int16,
        precipitationTenthMillimetres: UInt16, precipitationProbabilityPercent: UInt8,
        condition: OBCWeatherCondition, windFromDegrees: UInt16,
        windSpeedDeciMetresPerSecond: UInt16, windGustDeciMetresPerSecond: UInt16
    ) {
        self.validTimeOffsetSeconds = validTimeOffsetSeconds
        self.temperatureDeciCelsius = temperatureDeciCelsius
        self.precipitationTenthMillimetres = precipitationTenthMillimetres
        self.precipitationProbabilityPercent = precipitationProbabilityPercent
        self.condition = condition
        self.windFromDegrees = windFromDegrees
        self.windSpeedDeciMetresPerSecond = windSpeedDeciMetresPerSecond
        self.windGustDeciMetresPerSecond = windGustDeciMetresPerSecond
    }
}

public struct OBCWeatherQuality: OptionSet, Equatable, Sendable {
    public let rawValue: UInt32
    public init(rawValue: UInt32) { self.rawValue = rawValue }
    public static let observed = Self(rawValue: 1 << 0)
    public static let forecast = Self(rawValue: 1 << 1)
    public static let partialCoverage = Self(rawValue: 1 << 2)
    public static let degraded = Self(rawValue: 1 << 3)
}

public struct OBCWeatherRainFrame: Equatable, Sendable {
    public var validAtUnixSeconds: Int64
    public var width: UInt16
    public var height: UInt16
    public var cellSizeMetres: UInt16
    public var quality: OBCWeatherQuality
    /// Row-major 16 x 16 tiles; each inner array must contain exactly 256 intensity codes.
    public var tiles: [[UInt8]]

    public init(
        validAtUnixSeconds: Int64, width: UInt16, height: UInt16, cellSizeMetres: UInt16,
        quality: OBCWeatherQuality, tiles: [[UInt8]]
    ) {
        self.validAtUnixSeconds = validAtUnixSeconds
        self.width = width
        self.height = height
        self.cellSizeMetres = cellSizeMetres
        self.quality = quality
        self.tiles = tiles
    }
}

public struct OBCWeatherBundle: Equatable, Sendable {
    public var generation: UInt32
    public var requestID: UInt32
    public var generatedAtUnixSeconds: Int64
    /// Base timestamp of hourly record zero; genuine observed rain may precede it.
    public var validFromUnixSeconds: Int64
    /// Overall upper validity ceiling for hourly interval ends and rain timestamps.
    public var validUntilUnixSeconds: Int64
    public var bounds: OBCWeatherBounds
    public var hourly: [OBCWeatherHourlyRecord]
    public var rainFrames: [OBCWeatherRainFrame]

    public init(
        generation: UInt32, requestID: UInt32, generatedAtUnixSeconds: Int64,
        validFromUnixSeconds: Int64, validUntilUnixSeconds: Int64, bounds: OBCWeatherBounds,
        hourly: [OBCWeatherHourlyRecord], rainFrames: [OBCWeatherRainFrame]
    ) {
        self.generation = generation
        self.requestID = requestID
        self.generatedAtUnixSeconds = generatedAtUnixSeconds
        self.validFromUnixSeconds = validFromUnixSeconds
        self.validUntilUnixSeconds = validUntilUnixSeconds
        self.bounds = bounds
        self.hourly = hourly
        self.rainFrames = rainFrames
    }
}

/// Independent Swift implementation of `specs/OBCW_Spec.md`.
public enum OBCWeatherCodec {
    public static let producerPolicyMaximumLength = 65_536

    private static let magic = Data("OBCW".utf8)
    private static let version: UInt16 = 1
    private static let headerLength = 112
    private static let hourlyCount = 24
    private static let hourlyRecordLength = 24
    private static let hourlyIntervalSeconds: UInt32 = 3_600
    private static let frameDescriptorLength = 48
    private static let tileDirectoryEntryLength = 12
    private static let tileEdge = OBCPrecipitationTileCodec.tileEdge
    private static let tileCells = OBCPrecipitationTileCodec.tileCells
    private static let raw4Length = OBCPrecipitationTileCodec.raw4Length
    private static let crcOffset = 88

    private struct FrameLayout {
        var directoryOffset: Int
        var dataOffset: Int
        var dataLength: Int
        var tileLengths: [Int]
    }

    /// Phone producer entry point. The 64 KiB cap is policy, not a decode/format limit.
    public static func encode(_ bundle: OBCWeatherBundle) throws -> Data {
        let data = try encodeFormat(bundle)
        guard data.count <= producerPolicyMaximumLength else {
            throw OBCWeatherWireError.producerPolicyExceeded
        }
        return data
    }

    /// Encode the `uint32`-capacity format without applying phone producer policy.
    public static func encodeFormat(_ bundle: OBCWeatherBundle) throws -> Data {
        try validate(bundle)
        let frameBase = try checkedAdd(headerLength, try checkedMultiply(hourlyCount, hourlyRecordLength))
        var tail = try checkedAdd(
            frameBase, try checkedMultiply(bundle.rainFrames.count, frameDescriptorLength))
        var layouts: [FrameLayout] = []
        layouts.reserveCapacity(bundle.rainFrames.count)
        for frame in bundle.rainFrames {
            let lengths = try frame.tiles.map(OBCPrecipitationTileCodec.encodedLength)
            let directoryLength = try checkedMultiply(frame.tiles.count, tileDirectoryEntryLength)
            let dataOffset = try checkedAdd(tail, directoryLength)
            var dataLength = 0
            for length in lengths { dataLength = try checkedAdd(dataLength, length) }
            layouts.append(FrameLayout(
                directoryOffset: tail, dataOffset: dataOffset, dataLength: dataLength,
                tileLengths: lengths))
            tail = try checkedAdd(dataOffset, dataLength)
        }
        let totalLength = try checkedUInt32(tail)
        let frameBaseWire = try checkedUInt32(frameBase)
        let frameCountWire = try checkedUInt16(bundle.rainFrames.count)
        var data = Data(count: tail)
        data.replaceSubrange(0..<4, with: magic)
        data.putLE(version, at: 4)
        data.putLE(UInt16(headerLength), at: 6)
        data.putLE(totalLength, at: 8)
        data.putLE(bundle.generation, at: 12)
        data.putLE(bundle.requestID, at: 16)
        data.putLE(bundle.generatedAtUnixSeconds, at: 20)
        data.putLE(bundle.validFromUnixSeconds, at: 28)
        data.putLE(bundle.validUntilUnixSeconds, at: 36)
        data.putLE(bundle.bounds.southLatitudeMicrodegrees, at: 44)
        data.putLE(bundle.bounds.westLongitudeMicrodegrees, at: 48)
        data.putLE(bundle.bounds.northLatitudeMicrodegrees, at: 52)
        data.putLE(bundle.bounds.eastLongitudeMicrodegrees, at: 56)
        data.putLE(bundle.bounds.gridOriginLatitudeMicrodegrees, at: 60)
        data.putLE(bundle.bounds.gridOriginLongitudeMicrodegrees, at: 64)
        data.putLE(try checkedUInt32(headerLength), at: 68)
        data.putLE(UInt16(hourlyCount), at: 72)
        data.putLE(UInt16(hourlyRecordLength), at: 74)
        data.putLE(frameBaseWire, at: 76)
        data.putLE(frameCountWire, at: 80)
        data.putLE(UInt16(frameDescriptorLength), at: 82)

        for (index, record) in bundle.hourly.enumerated() {
            let offset = headerLength + index * hourlyRecordLength
            data.putLE(record.validTimeOffsetSeconds, at: offset)
            data.putLE(record.temperatureDeciCelsius, at: offset + 4)
            data.putLE(record.precipitationTenthMillimetres, at: offset + 6)
            data[offset + 8] = record.precipitationProbabilityPercent
            data[offset + 9] = record.condition.rawValue
            data.putLE(record.windFromDegrees, at: offset + 10)
            data.putLE(record.windSpeedDeciMetresPerSecond, at: offset + 12)
            data.putLE(record.windGustDeciMetresPerSecond, at: offset + 14)
        }

        for (index, frame) in bundle.rainFrames.enumerated() {
            let layout = layouts[index]
            let descriptor = frameBase + index * frameDescriptorLength
            data.putLE(frame.validAtUnixSeconds, at: descriptor)
            data.putLE(frame.width, at: descriptor + 8)
            data.putLE(frame.height, at: descriptor + 10)
            data.putLE(frame.cellSizeMetres, at: descriptor + 12)
            data[descriptor + 14] = UInt8(tileEdge)
            data.putLE(try checkedUInt32(layout.directoryOffset), at: descriptor + 16)
            data.putLE(try checkedUInt32(frame.tiles.count), at: descriptor + 20)
            data.putLE(try checkedUInt32(layout.dataOffset), at: descriptor + 24)
            data.putLE(try checkedUInt32(layout.dataLength), at: descriptor + 28)
            data.putLE(frame.quality.rawValue, at: descriptor + 32)

            var payload = layout.dataOffset
            for (tileIndex, tile) in frame.tiles.enumerated() {
                let length = layout.tileLengths[tileIndex]
                let encoding = try OBCPrecipitationTileCodec.encode(tile)
                guard encoding.bytes.count == length else { throw OBCWeatherWireError.malformed }
                let entry = layout.directoryOffset + tileIndex * tileDirectoryEntryLength
                data.putLE(try checkedUInt32(payload), at: entry)
                data.putLE(try checkedUInt16(length), at: entry + 4)
                data.putLE(UInt16(tileCells), at: entry + 6)
                data[entry + 8] = encoding.codec
                data.replaceSubrange(payload..<payload + length, with: encoding.bytes)
                payload += length
            }
            guard payload == layout.dataOffset + layout.dataLength else { throw OBCWeatherWireError.malformed }
        }
        data.putLE(UInt32(0), at: crcOffset)
        data.putLE(CRC32.checksum(data), at: crcOffset)
        return data
    }

    public static func decode(_ data: Data) throws -> OBCWeatherBundle {
        guard data.count >= headerLength, UInt64(data.count) <= UInt64(UInt32.max) else {
            throw OBCWeatherWireError.malformed
        }
        let availableLength = try checkedUInt32(data.count)
        guard data.readBytes(at: 0, count: 4) == magic,
              try require(data.readUInt16LE(at: 4)) == version,
              try require(data.readUInt16LE(at: 6)) == UInt16(headerLength),
              try require(data.readUInt32LE(at: 8)) == availableLength,
              try require(data.readUInt32LE(at: 68)) == UInt32(headerLength),
              try require(data.readUInt16LE(at: 72)) == UInt16(hourlyCount),
              try require(data.readUInt16LE(at: 74)) == UInt16(hourlyRecordLength),
              try require(data.readUInt32LE(at: 76)) == UInt32(headerLength + hourlyCount * hourlyRecordLength),
              try require(data.readUInt16LE(at: 82)) == UInt16(frameDescriptorLength),
              try require(data.readUInt32LE(at: 84)) == 0,
              data.allZero(in: 92..<112)
        else { throw OBCWeatherWireError.malformed }

        let storedCRC = try require(data.readUInt32LE(at: crcOffset))
        var hasher = CRC32.Hasher()
        hasher.update(data.prefix(crcOffset))
        hasher.update([UInt8](repeating: 0, count: 4))
        hasher.update(data.dropFirst(crcOffset + 4))
        guard hasher.finalize() == storedCRC else { throw OBCWeatherWireError.crcMismatch }

        let generatedAt = try require(data.readInt64LE(at: 20))
        let validFrom = try require(data.readInt64LE(at: 28))
        let validUntil = try require(data.readInt64LE(at: 36))
        let bounds = OBCWeatherBounds(
            southLatitudeMicrodegrees: try require(data.readInt32LE(at: 44)),
            westLongitudeMicrodegrees: try require(data.readInt32LE(at: 48)),
            northLatitudeMicrodegrees: try require(data.readInt32LE(at: 52)),
            eastLongitudeMicrodegrees: try require(data.readInt32LE(at: 56)),
            gridOriginLatitudeMicrodegrees: try require(data.readInt32LE(at: 60)),
            gridOriginLongitudeMicrodegrees: try require(data.readInt32LE(at: 64)))
        guard validHeader(generatedAt: generatedAt, validFrom: validFrom, validUntil: validUntil, bounds: bounds)
        else { throw OBCWeatherWireError.malformed }

        var hourly: [OBCWeatherHourlyRecord] = []
        hourly.reserveCapacity(hourlyCount)
        for index in 0..<hourlyCount {
            let offset = headerLength + index * hourlyRecordLength
            guard data.allZero(in: offset + 16..<offset + 24),
                  let conditionRaw = data.readUInt8(at: offset + 9),
                  let condition = OBCWeatherCondition(rawValue: conditionRaw)
            else { throw OBCWeatherWireError.malformed }
            let record = OBCWeatherHourlyRecord(
                validTimeOffsetSeconds: try require(data.readUInt32LE(at: offset)),
                temperatureDeciCelsius: try require(data.readInt16LE(at: offset + 4)),
                precipitationTenthMillimetres: try require(data.readUInt16LE(at: offset + 6)),
                precipitationProbabilityPercent: try require(data.readUInt8(at: offset + 8)),
                condition: condition,
                windFromDegrees: try require(data.readUInt16LE(at: offset + 10)),
                windSpeedDeciMetresPerSecond: try require(data.readUInt16LE(at: offset + 12)),
                windGustDeciMetresPerSecond: try require(data.readUInt16LE(at: offset + 14)))
            guard valid(record) else { throw OBCWeatherWireError.malformed }
            _ = try validateHourlyTime(
                index: index, record: record, validFrom: validFrom, validUntil: validUntil)
            hourly.append(record)
        }

        let frameCount = Int(try require(data.readUInt16LE(at: 80)))
        let frameBase = headerLength + hourlyCount * hourlyRecordLength
        var cursor = try checkedAdd(frameBase, try checkedMultiply(frameCount, frameDescriptorLength))
        guard cursor <= data.count else { throw OBCWeatherWireError.malformed }
        var priorFrame: Int64?
        var frames: [OBCWeatherRainFrame] = []
        frames.reserveCapacity(frameCount)
        for frameIndex in 0..<frameCount {
            let descriptor = frameBase + frameIndex * frameDescriptorLength
            guard data.allZero(in: descriptor + 15..<descriptor + 16),
                  data.allZero(in: descriptor + 36..<descriptor + 48),
                  try require(data.readUInt8(at: descriptor + 14)) == UInt8(tileEdge)
            else { throw OBCWeatherWireError.malformed }
            let validAt = try require(data.readInt64LE(at: descriptor))
            let width = try require(data.readUInt16LE(at: descriptor + 8))
            let height = try require(data.readUInt16LE(at: descriptor + 10))
            let cellSize = try require(data.readUInt16LE(at: descriptor + 12))
            let directoryOffset = Int(try require(data.readUInt32LE(at: descriptor + 16)))
            let tileCount = Int(try require(data.readUInt32LE(at: descriptor + 20)))
            let dataOffset = Int(try require(data.readUInt32LE(at: descriptor + 24)))
            let dataLength = Int(try require(data.readUInt32LE(at: descriptor + 28)))
            let quality = OBCWeatherQuality(rawValue: try require(data.readUInt32LE(at: descriptor + 32)))
            guard validAt > 0, validAt <= validUntil,
                  priorFrame.map({ validAt > $0 }) ?? true,
                  width > 0, height > 0, cellSize > 0, valid(quality),
                  expectedTileCount(width: width, height: height) == tileCount,
                  directoryOffset == cursor
            else { throw OBCWeatherWireError.malformed }
            priorFrame = validAt
            let directoryLength = try checkedMultiply(tileCount, tileDirectoryEntryLength)
            let expectedDataOffset = try checkedAdd(directoryOffset, directoryLength)
            guard dataOffset == expectedDataOffset else {
                throw OBCWeatherWireError.malformed
            }
            var payload = dataOffset
            var tiles: [[UInt8]] = []
            tiles.reserveCapacity(tileCount)
            for tileIndex in 0..<tileCount {
                let entry = try checkedAdd(directoryOffset, try checkedMultiply(tileIndex, tileDirectoryEntryLength))
                guard data.allZero(in: entry + 9..<entry + 12),
                      Int(try require(data.readUInt32LE(at: entry))) == payload,
                      try require(data.readUInt16LE(at: entry + 6)) == UInt16(tileCells),
                      let codec = data.readUInt8(at: entry + 8)
                else { throw OBCWeatherWireError.malformed }
                let encodedLength = Int(try require(data.readUInt16LE(at: entry + 4)))
                guard encodedLength > 0, encodedLength <= raw4Length,
                      let encoded = data.readBytes(at: payload, count: encodedLength)
                else { throw OBCWeatherWireError.malformed }
                let cells = try OBCPrecipitationTileCodec.decode(codec: codec, encoded: encoded)
                guard validPadding(cells, width: width, height: height, tileIndex: tileIndex) else {
                    throw OBCWeatherWireError.malformed
                }
                tiles.append(cells)
                payload = try checkedAdd(payload, encodedLength)
            }
            let expectedPayloadEnd = try checkedAdd(dataOffset, dataLength)
            guard payload == expectedPayloadEnd else { throw OBCWeatherWireError.malformed }
            cursor = payload
            frames.append(OBCWeatherRainFrame(
                validAtUnixSeconds: validAt, width: width, height: height,
                cellSizeMetres: cellSize, quality: quality, tiles: tiles))
        }
        guard cursor == data.count else { throw OBCWeatherWireError.malformed }

        return OBCWeatherBundle(
            generation: try require(data.readUInt32LE(at: 12)),
            requestID: try require(data.readUInt32LE(at: 16)),
            generatedAtUnixSeconds: generatedAt, validFromUnixSeconds: validFrom,
            validUntilUnixSeconds: validUntil, bounds: bounds, hourly: hourly, rainFrames: frames)
    }

    private static func validate(_ bundle: OBCWeatherBundle) throws {
        guard bundle.hourly.count == hourlyCount,
              bundle.rainFrames.count <= Int(UInt16.max),
              validHeader(
                generatedAt: bundle.generatedAtUnixSeconds, validFrom: bundle.validFromUnixSeconds,
                validUntil: bundle.validUntilUnixSeconds, bounds: bundle.bounds)
        else { throw OBCWeatherWireError.malformed }
        for (index, record) in bundle.hourly.enumerated() {
            guard valid(record) else { throw OBCWeatherWireError.malformed }
            _ = try validateHourlyTime(
                index: index, record: record, validFrom: bundle.validFromUnixSeconds,
                validUntil: bundle.validUntilUnixSeconds)
        }
        var priorFrame: Int64?
        for frame in bundle.rainFrames {
            guard frame.validAtUnixSeconds > 0,
                  frame.validAtUnixSeconds <= bundle.validUntilUnixSeconds,
                  priorFrame.map({ frame.validAtUnixSeconds > $0 }) ?? true,
                  frame.cellSizeMetres > 0, valid(frame.quality),
                  expectedTileCount(width: frame.width, height: frame.height) == frame.tiles.count,
                  frame.tiles.enumerated().allSatisfy({ index, tile in
                      tile.count == tileCells && tile.allSatisfy(OBCPrecipitationTileCodec.validIntensity)
                          && validPadding(tile, width: frame.width, height: frame.height, tileIndex: index)
                  })
            else { throw OBCWeatherWireError.malformed }
            priorFrame = frame.validAtUnixSeconds
        }
    }

    private static func validHeader(
        generatedAt: Int64, validFrom: Int64, validUntil: Int64, bounds: OBCWeatherBounds
    ) -> Bool {
        generatedAt > 0 && validUntil >= validFrom
            && (-90_000_000...90_000_000).contains(bounds.southLatitudeMicrodegrees)
            && (-90_000_000...90_000_000).contains(bounds.northLatitudeMicrodegrees)
            && (-180_000_000...180_000_000).contains(bounds.westLongitudeMicrodegrees)
            && (-180_000_000...180_000_000).contains(bounds.eastLongitudeMicrodegrees)
            && bounds.southLatitudeMicrodegrees < bounds.northLatitudeMicrodegrees
            && bounds.westLongitudeMicrodegrees < bounds.eastLongitudeMicrodegrees
            && bounds.gridOriginLatitudeMicrodegrees == bounds.southLatitudeMicrodegrees
            && bounds.gridOriginLongitudeMicrodegrees == bounds.westLongitudeMicrodegrees
    }

    private static func valid(_ record: OBCWeatherHourlyRecord) -> Bool {
        (record.temperatureDeciCelsius == Int16.min || (-1_000...700).contains(record.temperatureDeciCelsius))
            && (record.precipitationProbabilityPercent == UInt8.max
                || record.precipitationProbabilityPercent <= 100)
            && (record.windFromDegrees == UInt16.max || record.windFromDegrees <= 359)
            && (record.windSpeedDeciMetresPerSecond == UInt16.max
                || record.windSpeedDeciMetresPerSecond <= 2_000)
            && (record.windGustDeciMetresPerSecond == UInt16.max
                || record.windGustDeciMetresPerSecond <= 2_000)
    }

    private static func valid(_ quality: OBCWeatherQuality) -> Bool {
        let known: UInt32 = 0x0F
        let source = quality.rawValue & 0x03
        return quality.rawValue & ~known == 0 && (source == 1 || source == 2)
    }

    private static func expectedTileCount(width: UInt16, height: UInt16) -> Int? {
        guard width > 0, height > 0 else { return nil }
        let columns = (Int(width) + tileEdge - 1) / tileEdge
        let rows = (Int(height) + tileEdge - 1) / tileEdge
        return try? checkedMultiply(columns, rows)
    }

    private static func validPadding(_ cells: [UInt8], width: UInt16, height: UInt16, tileIndex: Int) -> Bool {
        guard cells.count == tileCells, let count = expectedTileCount(width: width, height: height),
              tileIndex >= 0, tileIndex < count else { return false }
        let tileColumns = (Int(width) + tileEdge - 1) / tileEdge
        let tileRow = tileIndex / tileColumns, tileColumn = tileIndex % tileColumns
        let validRows = min(tileEdge, max(0, Int(height) - tileRow * tileEdge))
        let validColumns = min(tileEdge, max(0, Int(width) - tileColumn * tileEdge))
        for row in 0..<tileEdge {
            for column in 0..<tileEdge where (row >= validRows || column >= validColumns) {
                if cells[row * tileEdge + column] != 15 { return false }
            }
        }
        return true
    }

    private static func checkedAdd(_ lhs: Int, _ rhs: Int) throws -> Int {
        let (value, overflow) = lhs.addingReportingOverflow(rhs)
        guard !overflow, value >= 0 else { throw OBCWeatherWireError.malformed }
        return value
    }

    private static func checkedMultiply(_ lhs: Int, _ rhs: Int) throws -> Int {
        let (value, overflow) = lhs.multipliedReportingOverflow(by: rhs)
        guard !overflow, value >= 0 else { throw OBCWeatherWireError.malformed }
        return value
    }

    private static func checkedUInt16(_ value: Int) throws -> UInt16 {
        guard value >= 0, UInt64(value) <= UInt64(UInt16.max) else { throw OBCWeatherWireError.malformed }
        return UInt16(value)
    }

    private static func checkedUInt32(_ value: Int) throws -> UInt32 {
        guard value >= 0, UInt64(value) <= UInt64(UInt32.max) else { throw OBCWeatherWireError.malformed }
        return UInt32(value)
    }

    @discardableResult
    private static func validateHourlyTime(
        index: Int, record: OBCWeatherHourlyRecord, validFrom: Int64, validUntil: Int64
    ) throws -> Int64 {
        let expectedOffset = try checkedUInt32(try checkedMultiply(index, Int(hourlyIntervalSeconds)))
        guard record.validTimeOffsetSeconds == expectedOffset else { throw OBCWeatherWireError.malformed }
        let (validAt, startOverflow) = validFrom.addingReportingOverflow(Int64(record.validTimeOffsetSeconds))
        let (intervalEnd, endOverflow) = validAt.addingReportingOverflow(Int64(hourlyIntervalSeconds))
        guard !startOverflow, !endOverflow, intervalEnd <= validUntil else {
            throw OBCWeatherWireError.malformed
        }
        return validAt
    }

    private static func require<T>(_ value: T?) throws -> T {
        guard let value else { throw OBCWeatherWireError.malformed }
        return value
    }
}

// Little-endian reads/writes shared by the module's codecs. Internal, never public: the wire
// contract is the codec API, not these accessors.
extension Data {
    func readUInt8(at offset: Int) -> UInt8? {
        guard offset >= 0, offset < count else { return nil }
        return self[startIndex + offset]
    }

    func readBytes(at offset: Int, count length: Int) -> Data? {
        guard offset >= 0, length >= 0 else { return nil }
        let (end, overflow) = offset.addingReportingOverflow(length)
        guard !overflow, end <= count else { return nil }
        return Data(self[(startIndex + offset)..<(startIndex + end)])
    }

    func readUInt16LE(at offset: Int) -> UInt16? {
        guard let bytes = readBytes(at: offset, count: 2) else { return nil }
        return bytes.withUnsafeBytes { UInt16(littleEndian: $0.loadUnaligned(as: UInt16.self)) }
    }

    func readInt16LE(at offset: Int) -> Int16? {
        readUInt16LE(at: offset).map { Int16(bitPattern: $0) }
    }

    func readUInt32LE(at offset: Int) -> UInt32? {
        guard let bytes = readBytes(at: offset, count: 4) else { return nil }
        return bytes.withUnsafeBytes { UInt32(littleEndian: $0.loadUnaligned(as: UInt32.self)) }
    }

    func readInt32LE(at offset: Int) -> Int32? {
        readUInt32LE(at: offset).map { Int32(bitPattern: $0) }
    }

    func readInt64LE(at offset: Int) -> Int64? {
        guard let bytes = readBytes(at: offset, count: 8) else { return nil }
        return bytes.withUnsafeBytes { Int64(littleEndian: $0.loadUnaligned(as: Int64.self)) }
    }

    func allZero(in range: Range<Int>) -> Bool {
        guard let bytes = readBytes(at: range.lowerBound, count: range.count) else { return false }
        return bytes.allSatisfy { $0 == 0 }
    }

    mutating func putLE(_ value: UInt16, at offset: Int) {
        self[startIndex + offset] = UInt8(value & 0xFF)
        self[startIndex + offset + 1] = UInt8(value >> 8)
    }

    mutating func putLE(_ value: Int16, at offset: Int) { putLE(UInt16(bitPattern: value), at: offset) }

    mutating func putLE(_ value: UInt32, at offset: Int) {
        for byte in 0..<4 { self[startIndex + offset + byte] = UInt8(truncatingIfNeeded: value >> (byte * 8)) }
    }

    mutating func putLE(_ value: Int32, at offset: Int) { putLE(UInt32(bitPattern: value), at: offset) }

    mutating func putLE(_ value: Int64, at offset: Int) {
        let bits = UInt64(bitPattern: value)
        for byte in 0..<8 { self[startIndex + offset + byte] = UInt8(truncatingIfNeeded: bits >> (byte * 8)) }
    }
}

// OBCWeatherWire cannot depend on OBCTransport just to share CRC code. This independent table is
// pinned by the same check value and golden objects; keeping the target below transport is the
// architecture boundary, not a second wire contract. OBCW and OBCG share this one implementation.
enum CRC32 {
    private static let table: [UInt32] = (0..<256).map { value in
        var crc = UInt32(value)
        for _ in 0..<8 { crc = crc & 1 == 1 ? 0xEDB8_8320 ^ (crc >> 1) : crc >> 1 }
        return crc
    }

    static func checksum<C: Collection>(_ bytes: C) -> UInt32 where C.Element == UInt8 {
        var hasher = Hasher(); hasher.update(bytes); return hasher.finalize()
    }

    struct Hasher {
        private var crc: UInt32 = 0xFFFF_FFFF
        mutating func update<C: Collection>(_ bytes: C) where C.Element == UInt8 {
            for byte in bytes { crc = CRC32.table[Int((crc ^ UInt32(byte)) & 0xFF)] ^ (crc >> 8) }
        }
        func finalize() -> UInt32 { crc ^ 0xFFFF_FFFF }
    }
}
