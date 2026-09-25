import Foundation
import OBCDomain

/// Ride object v6: verbatim 20-byte recorded samples followed by one fixed 154-byte `OBRF` footer.
public enum RideObjectCodec {
    static let version: UInt8 = 6
    static let sampleLength = 20
    static let footerLength = 154
    static let nameCapacity = 48
    static let noSensorU8: UInt8 = 0xFF
    static let noSensorU16: UInt16 = 0xFFFF
    static let noEnergy: UInt32 = 0xFFFF_FFFF

    public static func encode(_ ride: Ride) -> Data {
        let summary = ride.summary
        let name = clippedUTF8(summary.name, capacity: nameCapacity)
        var data = Data(capacity: ride.points.count * sampleLength + footerLength)
        let start = summary.date.timeIntervalSince1970

        for point in ride.points {
            data.appendLE(Int32(clamping: Int64((point.coordinate.longitude * 1e6).rounded())))
            data.appendLE(Int32(clamping: Int64((point.coordinate.latitude * 1e6).rounded())))
            data.appendLE(point.elevationMeters.map { Int16(clamping: Int64($0.rounded())) } ?? 0)
            data.appendLE(UInt16(point.segmentStart ? 1 : 0))
            let elapsedMs = ((point.timestamp.timeIntervalSince1970 - start) * 1_000).rounded()
            data.appendLE(UInt32(clamping: Int64(max(0, elapsedMs))))
            data.append(sensorU8(point.heartRate))
            data.append(sensorU8(point.cadence))
            data.appendLE(sensorU16(point.power))
        }

        data.append(contentsOf: [0x4F, 0x42, 0x52, 0x46, version, UInt8(name.count)])
        data.appendLE(UInt16(footerLength))
        data.appendLE(UInt32(clamping: Int64(start.rounded())))
        data.appendLE(UInt32(clamping: Int64(summary.distanceMeters.rounded())))
        data.appendLE(UInt32(clamping: Int64(summary.movingTime.rounded())))
        data.appendLE(UInt16(clamping: Int64((summary.averageSpeedMps * 100).rounded())))
        data.appendLE(UInt16(clamping: Int64(summary.climbMeters.rounded())))
        data.appendLE(UInt16(clamping: Int64(summary.descentMeters.rounded())))
        data.appendLE(UInt32(ride.points.count))
        data.append(sensorU8(summary.avgHeartRate))
        data.append(sensorU8(summary.maxHeartRate))
        data.append(sensorU8(summary.avgCadence))
        data.append(0)
        data.appendLE(sensorU16(summary.avgPower))
        data.appendLE(sensorU16(summary.maxPower))
        data.appendLE(summary.energyKJ.map { UInt32(clamping: Swift.min($0, Int(noEnergy) - 1)) } ?? noEnergy)
        data.append(name)
        data.append(Data(repeating: 0, count: nameCapacity - name.count))
        let trip = summary.trip
        let tripName = clippedUTF8(trip?.name ?? "", capacity: nameCapacity)
        data.appendLE(trip?.key ?? 0)
        data.append(UInt8(clamping: trip?.dayIndex ?? 0))
        data.append(UInt8(clamping: trip?.dayCount ?? 0))
        data.append(summary.bikeType.rawValue)
        data.append(UInt8(tripName.count))
        data.append(tripName)
        data.append(Data(repeating: 0, count: nameCapacity - tripName.count))
        data.append(limitU8(ride.zoneLimits.maxHeartRate))
        data.append(0)
        data.appendLE(limitU16(ride.zoneLimits.ftpWatts))
        return data
    }

    public static func decode(_ data: Data, id: RideID) throws -> Ride {
        guard data.count >= footerLength else { throw DeviceError.readFailed }
        let footerOffset = data.count - footerLength
        var footer = LEReader(Data(data[footerOffset...]))
        let magic = try footer.bytes(4)
        let decodedVersion = try footer.u8()
        guard magic == Data([0x4F, 0x42, 0x52, 0x46]), decodedVersion == version else {
            throw DeviceError.readFailed
        }
        let nameLength = Int(try footer.u8())
        guard nameLength <= nameCapacity, try footer.u16() == footerLength else {
            throw DeviceError.readFailed
        }
        let start = Date(timeIntervalSince1970: TimeInterval(try footer.u32()))
        let distance = Double(try footer.u32())
        let movingTime = TimeInterval(try footer.u32())
        let averageSpeed = Double(try footer.u16()) / 100
        let climb = Double(try footer.u16())
        let descent = Double(try footer.u16())
        let pointCount = Int(try footer.u32())
        let avgHR = optSensorU8(try footer.u8())
        let maxHR = optSensorU8(try footer.u8())
        let avgCadence = optSensorU8(try footer.u8())
        guard try footer.u8() == 0 else { throw DeviceError.readFailed }
        let avgPower = optSensorU16(try footer.u16())
        let maxPower = optSensorU16(try footer.u16())
        let energy = try footer.u32()
        let name = try nameField(&footer, length: nameLength)
        let tripKey = try footer.u64()
        let dayIndex = Int(try footer.u8())
        let dayCount = Int(try footer.u8())
        guard let bikeType = BikeType(rawValue: try footer.u8()) else { throw DeviceError.readFailed }
        let tripName = try nameField(&footer, length: Int(try footer.u8()))
        let maxHeartRateLimit = try footer.u8()
        guard try footer.u8() == 0 else { throw DeviceError.readFailed }
        let ftpLimit = try footer.u16()
        let zoneLimits = RideZoneLimits(maxHeartRate: maxHeartRateLimit == 0 ? nil : Int(maxHeartRateLimit),
                                        ftpWatts: ftpLimit == 0 ? nil : Int(ftpLimit))
        let trip: RideTrip?
        if tripKey == 0 {
            guard dayIndex == 0, dayCount == 0, tripName.isEmpty else { throw DeviceError.readFailed }
            trip = nil
        } else {
            guard dayIndex < dayCount else { throw DeviceError.readFailed }
            trip = RideTrip(key: tripKey, dayIndex: dayIndex, dayCount: dayCount, name: tripName)
        }
        let sampleBytes = pointCount.multipliedReportingOverflow(by: sampleLength)
        guard !sampleBytes.overflow, sampleBytes.partialValue == footerOffset else {
            throw DeviceError.readFailed
        }

        var samples = LEReader(Data(data[..<footerOffset]))
        typealias RawSample = (time: UInt32, lon: Int32, lat: Int32, elevation: Int16,
                               segmentStart: Bool, heartRate: Int?, cadence: Int?, power: Int?)
        var raw: [RawSample] = []
        raw.reserveCapacity(pointCount)
        for _ in 0..<pointCount {
            let lon = try samples.i32()
            let lat = try samples.i32()
            let elevation = try samples.i16()
            let flags = try samples.u16()
            guard flags & ~1 == 0 else { throw DeviceError.readFailed }
            raw.append((try samples.u32(), lon, lat, elevation, flags & 1 != 0,
                        optSensorU8(try samples.u8()), optSensorU8(try samples.u8()),
                        optSensorU16(try samples.u16())))
        }
        let firstTimestamp = raw.first?.time ?? 0
        let points = raw.map { sample in
            var point = RidePoint(
                timestamp: start.addingTimeInterval(TimeInterval(sample.time &- firstTimestamp) / 1_000),
                coordinate: Coordinate(latitude: Double(sample.lat) / 1e6,
                                       longitude: Double(sample.lon) / 1e6),
                elevationMeters: Double(sample.elevation), heartRate: sample.heartRate,
                cadence: sample.cadence, power: sample.power)
            point.segmentStart = sample.segmentStart
            return point
        }
        let summary = RideSummary(
            id: id, name: name, date: start, distanceMeters: distance,
            movingTime: movingTime, averageSpeedMps: averageSpeed, climbMeters: climb, descentMeters: descent,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate)),
            avgHeartRate: avgHR, maxHeartRate: maxHR, avgCadence: avgCadence,
            avgPower: avgPower, maxPower: maxPower, energyKJ: energy == noEnergy ? nil : Int(energy),
            bikeType: bikeType, trip: trip, zoneLimits: zoneLimits)
        return Ride(summary: summary, points: points)
    }

    /// A zero-padded UTF-8 field of `nameCapacity` bytes, `length` of them used.
    private static func nameField(_ reader: inout LEReader, length: Int) throws -> String {
        let field = try reader.bytes(nameCapacity)
        guard length <= nameCapacity, field.dropFirst(length).allSatisfy({ $0 == 0 }),
              let name = String(data: field.prefix(length), encoding: .utf8) else {
            throw DeviceError.readFailed
        }
        return name
    }

    private static func clippedUTF8(_ value: String, capacity: Int) -> Data {
        var bytes = Array(value.utf8.prefix(capacity))
        while String(bytes: bytes, encoding: .utf8) == nil { bytes.removeLast() }
        return Data(bytes)
    }

    private static func sensorU8(_ value: Int?) -> UInt8 {
        guard let value else { return noSensorU8 }
        return UInt8(clamping: Swift.min(value, Int(noSensorU8) - 1))
    }

    private static func sensorU16(_ value: Int?) -> UInt16 {
        guard let value else { return noSensorU16 }
        return UInt16(clamping: Swift.min(value, Int(noSensorU16) - 1))
    }

    /// A limit on the wire: `0` is not set.
    private static func limitU8(_ value: Int?) -> UInt8 { UInt8(clamping: value ?? 0) }
    private static func limitU16(_ value: Int?) -> UInt16 { UInt16(clamping: value ?? 0) }

    private static func optSensorU8(_ raw: UInt8) -> Int? { raw == noSensorU8 ? nil : Int(raw) }
    private static func optSensorU16(_ raw: UInt16) -> Int? { raw == noSensorU16 ? nil : Int(raw) }
}

extension Data {
    fileprivate mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        Swift.withUnsafeBytes(of: value.littleEndian) { append(contentsOf: $0) }
    }
}

private struct LEReader {
    private let data: Data
    private var offset: Int = 0

    init(_ data: Data) { self.data = data }

    mutating func bytes(_ count: Int) throws -> Data {
        guard count >= 0, data.count - offset >= count else { throw DeviceError.readFailed }
        defer { offset += count }
        return data[offset..<(offset + count)]
    }

    mutating func u8() throws -> UInt8 { try fixed() }
    mutating func u16() throws -> UInt16 { try fixed() }
    mutating func u32() throws -> UInt32 { try fixed() }
    mutating func u64() throws -> UInt64 { try fixed() }
    mutating func i16() throws -> Int16 { try fixed() }
    mutating func i32() throws -> Int32 { try fixed() }

    private mutating func fixed<T: FixedWidthInteger>() throws -> T {
        let raw = try bytes(MemoryLayout<T>.size)
        var value: T = 0
        _ = withUnsafeMutableBytes(of: &value) { raw.copyBytes(to: $0) }
        return T(littleEndian: value)
    }
}
