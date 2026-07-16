import Foundation
import OBCDomain

/// The **ride object** codec (v1 **and** v2) — the compact-binary object a ride
/// crosses the wire as (B7), ratified byte-for-byte by firmware
/// (`obc-ble-interface-spec.md` §7.2; pinned against `protocol-vectors/ride-v{1,2}.bin`
/// by `ProtocolVectorTests` / `RideCodecV2Tests`). Public on purpose: the mock
/// encodes fixture rides with it and the sync flow decodes downloads through it,
/// so the app exercises the real decode path with no firmware.
///
/// The device serves whichever version it wrote the file as; the app **accepts
/// both** — a v1 ride recorded before the BLE-sensor epic (#707) must still
/// download, list, and delete. v2 is an additive object version (no protocol
/// bump, spec §1): it appends a per-ride sensor summary to the header and
/// per-point `hr`/`cad`/`pwr` to each record.
///
/// Layout (little-endian):
/// ```
/// version     u8   = 1 or 2
/// nameLen     u16  · name UTF-8
/// startTime   u32  unix seconds
/// distance    u32  metres
/// movingTime  u32  seconds
/// avgSpeed    u16  cm/s
/// climb       u16  metres
/// pointCount  u32
/// -- v2 only, the per-ride BLE-sensor summary: --
///   avgHR     u8   bpm · 0xFF   = none
///   maxHR     u8   bpm · 0xFF   = none
///   avgCad    u8   rpm · 0xFF   = none
///   pad       u8   = 0 (reserved, aligns the u16 power fields)
///   avgPwr    u16  W   · 0xFFFF = none
///   maxPwr    u16  W   · 0xFFFF = none
/// point × N (v1: 14 B · v2: 18 B):
///   tOffset   u32  seconds since startTime
///   lat, lon  i32  degrees × 1e7
///   ele       i16  metres · Int16.min = none
///   -- v2 only: --
///     hr      u8   bpm · 0xFF   = absent
///     cad     u8   rpm · 0xFF   = absent
///     pwr     u16  W   · 0xFFFF = absent
/// ```
/// The byte length is fully determined **per version** (v1 `23 + nameLen + 14·N`,
/// v2 `31 + nameLen + 18·N`); a payload whose length disagrees is rejected — the
/// firmware's power-cut / torn-write guard, mirrored here. Quantization (whole
/// seconds/metres, ~1 cm coordinates) is deliberate — the canonical `Ride` is
/// lossy of the wire object, never the reverse.
public enum RideObjectCodec {
    static let version1: UInt8 = 1
    static let version2: UInt8 = 2
    static let headerLengthV1 = 1 + 2 + 4 + 4 + 4 + 2 + 2 + 4  // 23
    static let headerLengthV2 = headerLengthV1 + 8             // 31
    static let pointLengthV1 = 14
    static let pointLengthV2 = 18
    /// `ele` sentinel for "no elevation recorded".
    static let noElevation = Int16.min
    /// Sentinel for an absent `hr` / `cad` (header or per-point) `u8` field.
    static let noSensorU8: UInt8 = 0xFF
    /// Sentinel for an absent `pwr` (header or per-point) `u16` field.
    static let noSensorU16: UInt16 = 0xFFFF

    /// The whole encoded object's size for a `version`, name length, and point count.
    static func objectLength(version: UInt8, nameLength: Int, pointCount: Int) -> Int {
        let header = version >= version2 ? headerLengthV2 : headerLengthV1
        let point = version >= version2 ? pointLengthV2 : pointLengthV1
        return header + nameLength + point * pointCount
    }

    /// A ride encodes as **v2** the moment it carries any sensor value (a header
    /// summary field or one per-point sample); otherwise it stays **v1**, so a
    /// sensor-less ride is byte-identical to what pre-#707 firmware wrote (and
    /// the v1 vector round-trips exactly).
    static func version(for ride: Ride) -> UInt8 {
        let s = ride.summary
        let headerHasSensors = s.avgHeartRate != nil || s.maxHeartRate != nil
            || s.avgCadence != nil || s.avgPower != nil || s.maxPower != nil
        let pointsHaveSensors = ride.points.contains {
            $0.heartRate != nil || $0.cadence != nil || $0.power != nil
        }
        return headerHasSensors || pointsHaveSensors ? version2 : version1
    }

    public static func encode(_ ride: Ride) -> Data {
        let ver = version(for: ride)
        let isV2 = ver >= version2
        let summary = ride.summary
        let name = Data(summary.name.utf8.prefix(Int(UInt16.max)))
        let capacity = objectLength(version: ver, nameLength: name.count, pointCount: ride.points.count)
        var data = Data(capacity: capacity)
        data.append(ver)
        data.appendLE(UInt16(name.count))
        data.append(name)
        let start = summary.date.timeIntervalSince1970
        data.appendLE(UInt32(clamping: Int64(start.rounded())))
        data.appendLE(UInt32(clamping: Int64(summary.distanceMeters.rounded())))
        data.appendLE(UInt32(clamping: Int64(summary.movingTime.rounded())))
        data.appendLE(UInt16(clamping: Int64((summary.averageSpeedMps * 100).rounded())))
        data.appendLE(UInt16(clamping: Int64(summary.climbMeters.rounded())))
        data.appendLE(UInt32(ride.points.count))
        if isV2 {
            data.append(sensorU8(summary.avgHeartRate))
            data.append(sensorU8(summary.maxHeartRate))
            data.append(sensorU8(summary.avgCadence))
            data.append(0)  // reserved pad, aligns the u16 power fields
            data.appendLE(sensorU16(summary.avgPower))
            data.appendLE(sensorU16(summary.maxPower))
        }
        for point in ride.points {
            data.appendLE(UInt32(clamping: Int64((point.timestamp.timeIntervalSince1970 - start).rounded())))
            data.appendLE(Int32(clamping: Int64((point.coordinate.latitude * 1e7).rounded())))
            data.appendLE(Int32(clamping: Int64((point.coordinate.longitude * 1e7).rounded())))
            let ele = point.elevationMeters.map { Int16(clamping: Int64($0.rounded())) } ?? noElevation
            data.appendLE(ele)
            if isV2 {
                data.append(sensorU8(point.heartRate))
                data.append(sensorU8(point.cadence))
                data.appendLE(sensorU16(point.power))
            }
        }
        return data
    }

    /// Decode a downloaded payload into the canonical `Ride`. Accepts **v1 and
    /// v2** by the version byte; a v1 payload decodes with every sensor field
    /// `nil`. The id comes from the transfer envelope (`DownloadedRide.id`), not
    /// the payload. Malformed bytes — an unknown version, or a length that
    /// disagrees with the version's fixed layout — throw `DeviceError.readFailed`,
    /// so the caller keeps the ride as summary-only rather than dropping it.
    public static func decode(_ data: Data, id: RideID) throws -> Ride {
        var reader = LEReader(data)
        let ver = try reader.u8()
        guard ver == version1 || ver == version2 else { throw DeviceError.readFailed }
        let isV2 = ver >= version2
        let nameLen = Int(try reader.u16())
        let name = String(decoding: try reader.bytes(nameLen), as: UTF8.self)
        let start = Date(timeIntervalSince1970: TimeInterval(try reader.u32()))
        let distance = Double(try reader.u32())
        let movingTime = TimeInterval(try reader.u32())
        let avgSpeed = Double(try reader.u16()) / 100
        let climb = Double(try reader.u16())
        let pointCount = Int(try reader.u32())

        var avgHR: Int? = nil, maxHR: Int? = nil, avgCad: Int? = nil
        var avgPwr: Int? = nil, maxPwr: Int? = nil
        if isV2 {
            avgHR = optSensorU8(try reader.u8())
            maxHR = optSensorU8(try reader.u8())
            avgCad = optSensorU8(try reader.u8())
            _ = try reader.u8()  // reserved pad
            avgPwr = optSensorU16(try reader.u16())
            maxPwr = optSensorU16(try reader.u16())
        }

        let pointLength = isV2 ? pointLengthV2 : pointLengthV1
        guard reader.remaining == pointCount * pointLength else { throw DeviceError.readFailed }

        var points: [RidePoint] = []
        points.reserveCapacity(pointCount)
        for _ in 0..<pointCount {
            let tOffset = TimeInterval(try reader.u32())
            let lat = Double(try reader.i32()) / 1e7
            let lon = Double(try reader.i32()) / 1e7
            let ele = try reader.i16()
            var hr: Int? = nil, cad: Int? = nil, pwr: Int? = nil
            if isV2 {
                hr = optSensorU8(try reader.u8())
                cad = optSensorU8(try reader.u8())
                pwr = optSensorU16(try reader.u16())
            }
            points.append(RidePoint(
                timestamp: start.addingTimeInterval(tOffset),
                coordinate: Coordinate(latitude: lat, longitude: lon),
                elevationMeters: ele == noElevation ? nil : Double(ele),
                heartRate: hr,
                cadence: cad,
                power: pwr
            ))
        }

        let summary = RideSummary(
            id: id, name: name, date: start,
            distanceMeters: distance, movingTime: movingTime,
            averageSpeedMps: avgSpeed, climbMeters: climb,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate)),
            avgHeartRate: avgHR, maxHeartRate: maxHR, avgCadence: avgCad,
            avgPower: avgPwr, maxPower: maxPwr
        )
        return Ride(summary: summary, points: points)
    }

    // MARK: Sentinel plumbing

    /// A present `u8` sensor value clamped below the `0xFF` sentinel (a real bpm /
    /// rpm never reaches it — matches the firmware's saturating u16→u8 HR); `nil`
    /// → the sentinel.
    private static func sensorU8(_ value: Int?) -> UInt8 {
        guard let value else { return noSensorU8 }
        return UInt8(clamping: Swift.min(value, Int(noSensorU8) - 1))
    }

    private static func sensorU16(_ value: Int?) -> UInt16 {
        guard let value else { return noSensorU16 }
        return UInt16(clamping: Swift.min(value, Int(noSensorU16) - 1))
    }

    private static func optSensorU8(_ raw: UInt8) -> Int? {
        raw == noSensorU8 ? nil : Int(raw)
    }

    private static func optSensorU16(_ raw: UInt16) -> Int? {
        raw == noSensorU16 ? nil : Int(raw)
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
