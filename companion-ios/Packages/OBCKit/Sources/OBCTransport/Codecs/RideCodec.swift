import Foundation
import OBCDomain

/// Ride object v3: verbatim 20-byte recorded samples followed by one fixed 84-byte `OBRF` footer.
public enum RideObjectCodec {
    static let version: UInt8 = 3
    static let sampleLength = 20
    static let footerLength = 84
    static let nameCapacity = 48
    static let noSensorU8: UInt8 = 0xFF
    static let noSensorU16: UInt16 = 0xFFFF

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
        data.appendLE(UInt32(ride.points.count))
        data.append(sensorU8(summary.avgHeartRate))
        data.append(sensorU8(summary.maxHeartRate))
        data.append(sensorU8(summary.avgCadence))
        data.append(0)
        data.appendLE(sensorU16(summary.avgPower))
        data.appendLE(sensorU16(summary.maxPower))
        data.append(name)
        data.append(Data(repeating: 0, count: nameCapacity - name.count))
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
        let pointCount = Int(try footer.u32())
        let avgHR = optSensorU8(try footer.u8())
        let maxHR = optSensorU8(try footer.u8())
        let avgCadence = optSensorU8(try footer.u8())
        guard try footer.u8() == 0 else { throw DeviceError.readFailed }
        let avgPower = optSensorU16(try footer.u16())
        let maxPower = optSensorU16(try footer.u16())
        let nameField = try footer.bytes(nameCapacity)
        let sampleBytes = pointCount.multipliedReportingOverflow(by: sampleLength)
        guard nameField.dropFirst(nameLength).allSatisfy({ $0 == 0 }),
              let name = String(data: nameField.prefix(nameLength), encoding: .utf8),
              !sampleBytes.overflow, sampleBytes.partialValue == footerOffset else {
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
            movingTime: movingTime, averageSpeedMps: averageSpeed, climbMeters: climb,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate)),
            avgHeartRate: avgHR, maxHeartRate: maxHR, avgCadence: avgCadence,
            avgPower: avgPower, maxPower: maxPower)
        return Ride(summary: summary, points: points)
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
    mutating func i16() throws -> Int16 { try fixed() }
    mutating func i32() throws -> Int32 { try fixed() }

    private mutating func fixed<T: FixedWidthInteger>() throws -> T {
        let raw = try bytes(MemoryLayout<T>.size)
        var value: T = 0
        _ = withUnsafeMutableBytes(of: &value) { raw.copyBytes(to: $0) }
        return T(littleEndian: value)
    }
}
