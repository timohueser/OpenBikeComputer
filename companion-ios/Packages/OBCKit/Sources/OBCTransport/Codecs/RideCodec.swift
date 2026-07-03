import Foundation
import OBCDomain

/// The **ride object v1** codec — the compact-binary object a ride crosses the
/// wire as, ratified byte-for-byte by firmware
/// (`obc-ble-interface-spec.md` §7.2; pinned against
/// `protocol-vectors/ride-v1.bin` by `ProtocolVectorTests`). Public on purpose:
/// the mock encodes fixture rides with it and the sync flow decodes downloads
/// through it, so the app exercises the real decode path with no firmware.
///
/// Layout (little-endian):
/// ```
/// version     u8   = 1
/// nameLen     u16  · name UTF-8
/// startTime   u32  unix seconds
/// distance    u32  metres
/// movingTime  u32  seconds
/// avgSpeed    u16  cm/s
/// climb       u16  metres
/// pointCount  u32
/// point × N (14 B):
///   tOffset   u32  seconds since startTime
///   lat, lon  i32  degrees × 1e7
///   ele       i16  metres · Int16.min = none
/// ```
/// Quantization (whole seconds/metres, ~1 cm coordinates) is deliberate — it's
/// what a compact MCU-side format costs; the canonical `Ride` is lossy of the
/// wire object, never the reverse.
public enum RideObjectCodec {
    static let version: UInt8 = 1
    static let headerLength = 1 + 2 + 4 + 4 + 4 + 2 + 2 + 4
    static let pointLength = 14
    /// `ele` sentinel for "no elevation recorded".
    static let noElevation = Int16.min

    public static func encode(_ ride: Ride) -> Data {
        let summary = ride.summary
        let name = Data(summary.name.utf8.prefix(Int(UInt16.max)))
        var data = Data(capacity: headerLength + name.count + ride.points.count * pointLength)
        data.append(version)
        data.appendLE(UInt16(name.count))
        data.append(name)
        let start = summary.date.timeIntervalSince1970
        data.appendLE(UInt32(clamping: Int64(start.rounded())))
        data.appendLE(UInt32(clamping: Int64(summary.distanceMeters.rounded())))
        data.appendLE(UInt32(clamping: Int64(summary.movingTime.rounded())))
        data.appendLE(UInt16(clamping: Int64((summary.averageSpeedMps * 100).rounded())))
        data.appendLE(UInt16(clamping: Int64(summary.climbMeters.rounded())))
        data.appendLE(UInt32(ride.points.count))
        for point in ride.points {
            data.appendLE(UInt32(clamping: Int64((point.timestamp.timeIntervalSince1970 - start).rounded())))
            data.appendLE(Int32(clamping: Int64((point.coordinate.latitude * 1e7).rounded())))
            data.appendLE(Int32(clamping: Int64((point.coordinate.longitude * 1e7).rounded())))
            let ele = point.elevationMeters.map { Int16(clamping: Int64($0.rounded())) } ?? noElevation
            data.appendLE(ele)
        }
        return data
    }

    /// Decode a downloaded payload into the canonical `Ride`. The id comes from
    /// the transfer envelope (`DownloadedRide.id`), not the payload. Malformed
    /// bytes throw `DeviceError.readFailed` — the caller keeps the ride as
    /// summary-only rather than dropping it.
    public static func decode(_ data: Data, id: RideID) throws -> Ride {
        var reader = LEReader(data)
        guard try reader.u8() == version else { throw DeviceError.readFailed }
        let nameLen = Int(try reader.u16())
        let name = String(decoding: try reader.bytes(nameLen), as: UTF8.self)
        let start = Date(timeIntervalSince1970: TimeInterval(try reader.u32()))
        let distance = Double(try reader.u32())
        let movingTime = TimeInterval(try reader.u32())
        let avgSpeed = Double(try reader.u16()) / 100
        let climb = Double(try reader.u16())
        let pointCount = Int(try reader.u32())
        guard reader.remaining == pointCount * pointLength else { throw DeviceError.readFailed }

        var points: [RidePoint] = []
        points.reserveCapacity(pointCount)
        for _ in 0..<pointCount {
            let tOffset = TimeInterval(try reader.u32())
            let lat = Double(try reader.i32()) / 1e7
            let lon = Double(try reader.i32()) / 1e7
            let ele = try reader.i16()
            points.append(RidePoint(
                timestamp: start.addingTimeInterval(tOffset),
                coordinate: Coordinate(latitude: lat, longitude: lon),
                elevationMeters: ele == noElevation ? nil : Double(ele)
            ))
        }

        let summary = RideSummary(
            id: id, name: name, date: start,
            distanceMeters: distance, movingTime: movingTime,
            averageSpeedMps: avgSpeed, climbMeters: climb,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate))
        )
        return Ride(summary: summary, points: points)
    }
}

// MARK: - Little-endian plumbing

extension Data {
    fileprivate mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        Swift.withUnsafeBytes(of: value.littleEndian) { append(contentsOf: $0) }
    }
}

/// Bounds-checked little-endian cursor over a payload — every under-run is a
/// `DeviceError.readFailed`, never a crash (device bytes are untrusted input).
private struct LEReader {
    private let data: Data
    private var offset: Int

    init(_ data: Data) {
        self.data = data
        self.offset = data.startIndex
    }

    var remaining: Int { data.endIndex - offset }

    mutating func bytes(_ count: Int) throws -> Data {
        guard count >= 0, remaining >= count else { throw DeviceError.readFailed }
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
