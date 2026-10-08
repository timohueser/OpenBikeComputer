import Foundation
import OBCDomain
import OBCHost

/// The Rust OBCR v5 encoder and the Swift decoder. Stored geometry owns the route totals.
/// Missing elevation and incoming graph validity stay explicit. The exact bytes are in specs/OBCR_Spec.md, and shared
/// Rust-produced vectors pin the reader.
public enum RouteObjectCodec {
    // MARK: Format constants

    static let magic = Data("OBCR".utf8)
    /// The one version the device accepts. Older files are rejected on both sides, so a stored
    /// route re-imports rather than mis-decoding.
    static let version: UInt8 = 5
    /// The header's ride core; every ride-path field lives here. The waypoint extension follows.
    static let headerBaseLength = 112
    static let headerLength = 160
    static let chunkMetaLength = 44
    static let waypointLength = 80
    /// First byte of a waypoint record's name field.
    static let waypointNameOffset = 20
    /// Header route-name cap (matches the device `NAME_CAP`).
    static let nameCap = 48
    /// Waypoint short-name cap.
    static let waypointNameCap = 24
    /// `INT16_MIN` = "elevation unknown" in a waypoint record.
    static let waypointElevationUnknown = Int16.min

    // MARK: Encode

    /// Encode an imported route's geometry and waypoints into an OBCR v5 file, named `name` and
    /// truncated to ``nameCap`` on a character boundary.
    public static func encode(_ route: ImportedRoute, name: String, bikeType: BikeType) -> Data {
        encode(points: route.points, waypoints: route.waypoints, name: name, bikeType: bikeType)
    }

    /// The header Total Distance, Total Ascent and Total Descent an upload of `points` carries:
    /// the figures the device shows and estimates from. Nil for geometry that does not encode.
    public static func totals(points: [RoutePoint]) -> (distanceMeters: UInt32, ascentMeters: UInt32, descentMeters: UInt32)? {
        totals(of: encode(points: points, waypoints: [], name: "", bikeType: .road))
    }

    /// The header totals of an encoded payload.
    static func totals(of payload: Data) -> (distanceMeters: UInt32, ascentMeters: UInt32, descentMeters: UInt32)? {
        let header = ByteView(payload)
        guard let distance = try? header.u32(at: 36), let ascent = try? header.u32(at: 40),
            let descent = try? header.u32(at: 44) else { return nil }
        return (distance, ascent, descent)
    }

    /// The CRC-32 of the payload an upload of this library record would send: the record's
    /// geometry and waypoints under its display name, exactly what the detail screen's upload blob
    /// encodes. One definition, which is what makes "up to date" mean byte-identical.
    public static func payloadCRC(for record: PlannedRouteRecord) -> UInt32 {
        CRC32.checksum(encode(
            points: record.route.points, waypoints: record.route.waypoints, name: record.summary.name,
            bikeType: record.bikeType))
    }

    /// `payload` with only its header name field replaced: the bytes a device holds for a route
    /// the rider has since renamed on the phone. The name is a fixed-width field, so geometry,
    /// offsets and length are untouched; this is a header splice, never a re-encode. A payload too
    /// short to carry a header comes back unchanged.
    public static func renamed(_ payload: Data, to name: String) -> Data {
        guard payload.count >= headerLength else { return payload }
        let nameBytes = Data(name.truncatedToUTF8Bytes(nameCap).utf8)
        var out = payload
        let base = out.startIndex
        out.resetBytes(in: (base + 64)..<(base + 64 + nameCap))
        out[base + 6] = UInt8(nameBytes.count)
        out.replaceSubrange((base + 64)..<(base + 64 + nameBytes.count), with: nameBytes)
        return out
    }

    /// Encode geometry and waypoints into an OBCR v5 file. `waypoints` are stored verbatim,
    /// already placed along the route; `points` carry the geometry and drive the header stats.
    /// Invalid, empty or unsupported input yields empty `Data`. No partial object is returned.
    public static func encode(points: [RoutePoint], waypoints: [Waypoint], name: String, bikeType: BikeType) -> Data {
        guard !points.isEmpty, points.allSatisfy({ $0.coordinate.isValidGeographic
            && ($0.elevationMeters?.isFinite ?? true) }),
              waypoints.allSatisfy({ $0.coordinate.isValidGeographic }) else { return Data() }
        let rawPoints = points.map { point in
            ObcRoutePoint(lon: toMicrodegrees(point.coordinate.longitude),
                lat: toMicrodegrees(point.coordinate.latitude),
                elevation: point.elevationMeters.map(roundToInt16) ?? Int16.min,
                surface: point.surface, elevation_incomplete: point.elevationIncomplete ? 1 : 0)
        }
        let rawWaypoints = waypoints.map { waypoint in
            var raw = ObcRouteWaypoint()
            raw.raw_distance_m = waypoint.distanceAlongMeters
            raw.lon = toMicrodegrees(waypoint.coordinate.longitude)
            raw.lat = toMicrodegrees(waypoint.coordinate.latitude)
            raw.lateral_offset_m = lateralOffsetInt16(waypoint.lateralOffsetMeters)
            raw.category = WaypointCategory.wireID(waypoint.category)
            let name = Array(waypoint.name.truncatedToUTF8Bytes(waypointNameCap).utf8)
            raw.name_len = UInt8(name.count)
            withUnsafeMutableBytes(of: &raw.name) { $0.copyBytes(from: name) }
            if let provenance = waypoint.provenance, provenance.store.count == 16 {
                raw.has_provenance = 1
                withUnsafeMutableBytes(of: &raw.store) { $0.copyBytes(from: provenance.store) }
                raw.object = provenance.object
                raw.revision = provenance.revision
                raw.ordinal = provenance.ordinal
            }
            return raw
        }
        let name = Array(name.truncatedToUTF8Bytes(nameCap).utf8)
        return rawPoints.withUnsafeBufferPointer { points in
            rawWaypoints.withUnsafeBufferPointer { waypoints in
                name.withUnsafeBufferPointer { name in
                    var error: Int32 = 0
                    guard let encoded = obc_format_route_encode(points.baseAddress, points.count,
                        waypoints.baseAddress, waypoints.count, name.baseAddress, name.count,
                        bikeType.rawValue, &error) else { return Data() }
                    defer { obc_format_route_free(encoded) }
                    guard let bytes = obc_format_route_data(encoded) else { return Data() }
                    return Data(bytes: bytes, count: obc_format_route_len(encoded))
                }
            }
        }
    }

    private static func lateralOffsetInt16(_ meters: Double) -> Int16 {
        guard meters.isFinite else { return 0 }
        let magnitude = min(Double(Int16.max), abs(meters).rounded())
        return Int16(meters < 0 ? -magnitude : magnitude)
    }

    // MARK: Decode

    /// The parsed contents of an OBCR file: the header stats, the deduped geometry with seams
    /// counted once, and the waypoints.
    public struct Decoded: Equatable, Sendable {
        public var name: String
        public var version: UInt8
        public var bikeType: BikeType
        /// Header point count, the distinct stored points. It exceeds `points.count` only if the
        /// file's stored count disagrees with its geometry.
        public var storedPointCount: UInt32
        public var totalDistanceMeters: UInt32
        public var totalAscentMeters: UInt32
        public var totalDescentMeters: UInt32
        public var minElevationMeters: Int16
        public var maxElevationMeters: Int16
        /// First route point, for camera centering.
        public var start: Coordinate
        /// The decoded polyline, seams deduplicated — every stored vertex once.
        public var points: [RoutePoint]
        public var waypoints: [Waypoint]
        public var unresolvedAvoidance: Bool
        public var attributionMap: Data?
        public var visitDescriptor: Data?
    }

    /// Decode an OBCR v5 file. Every section is reached by an explicit offset and bounds-checked,
    /// so malformed device bytes throw ``DeviceError/readFailed`` and never trap. An older file is
    /// rejected, not read: its waypoint records are a different width and its category byte a
    /// retired taxonomy, so the honest answer is "re-import it", exactly what the device says.
    public static func decode(_ data: Data) throws -> Decoded {
        let reader = ByteView(data)
        guard try reader.bytes(at: 0, count: 4) == magic else { throw DeviceError.readFailed }
        let version = try reader.u8(at: 4)
        guard version == RouteObjectCodec.version else { throw DeviceError.readFailed }
        guard data.count >= headerLength else { throw DeviceError.readFailed }
        let flags = try reader.u8(at: 5)
        guard flags & ~31 == 0, try reader.u8(at: 119) == 0,
            let bikeType = BikeType(rawValue: try reader.u8(at: 7)) else { throw DeviceError.readFailed }
        let mapBytes = try reader.bytes(at: 128, count: 32)
        if flags & 4 == 0 {
            guard mapBytes.allSatisfy({ $0 == 0 }) else { throw DeviceError.readFailed }
        } else {
            guard try reader.u64(at: 144) > 0, try reader.u64(at: 152) > 0 else { throw DeviceError.readFailed }
        }
        let descriptorVersion = try reader.u8(at: 118)
        let descriptorOffset = Int(try reader.u32(at: 120))
        let descriptorLength = Int(try reader.u32(at: 124))
        let descriptor: Data?
        if descriptorVersion == 0 {
            guard descriptorOffset == 0, descriptorLength == 0 else { throw DeviceError.readFailed }
            descriptor = nil
        } else {
            guard descriptorVersion == 1, descriptorOffset >= headerLength, descriptorLength == 80 else { throw DeviceError.readFailed }
            descriptor = try reader.bytes(at: descriptorOffset, count: descriptorLength)
            let visit = ByteView(descriptor!)
            guard try visit.u64(at: 16) > 0, try visit.u64(at: 24) > 0, try visit.u64(at: 56) > 0,
                (1...4).contains(try visit.u8(at: 72)), try visit.bytes(at: 73, count: 7).allSatisfy({ $0 == 0 }),
                (-180_000_000...180_000_000).contains(try visit.i32(at: 64)),
                (-90_000_000...90_000_000).contains(try visit.i32(at: 68)) else { throw DeviceError.readFailed }
            for base in [32, 44] {
                guard try visit.u32(at: base) <= visit.u32(at: base + 4), try visit.u32(at: base + 4) <= visit.u32(at: base + 8) else { throw DeviceError.readFailed }
            }
            guard try visit.u32(at: 52) <= reader.u32(at: 36) else { throw DeviceError.readFailed }
        }
        let nameLength = min(Int(try reader.u8(at: 6)), nameCap)

        let start = Coordinate(
            latitude: fromMicrodegrees(try reader.i32(at: 28)),
            longitude: fromMicrodegrees(try reader.i32(at: 24))
        )
        let storedPointCount = try reader.u32(at: 32)
        let totalDistance = try reader.u32(at: 36)
        let totalAscent = try reader.u32(at: 40)
        let totalDescent = try reader.u32(at: 44)
        let minElevation = try reader.i16(at: 48)
        let maxElevation = try reader.i16(at: 50)
        let chunkCount = Int(try reader.u32(at: 52))
        let indexOffset = Int(try reader.u32(at: 56))
        let name = String(decoding: try reader.bytes(at: 64, count: nameLength), as: UTF8.self)

        var waypointOffset = 0
        var waypointCount = 0
        if data.count >= headerLength {
            waypointOffset = Int(try reader.u32(at: 112))
            waypointCount = Int(try reader.u16(at: 116))
        }

        if descriptor != nil {
            // Wire offsets/counts are at most UInt32. Widen before range arithmetic.
            let start = UInt64(descriptorOffset)
            let end = start + UInt64(descriptorLength)
            for (offset, count, width) in [(indexOffset, chunkCount, chunkMetaLength), (waypointOffset, waypointCount, waypointLength)] {
                let tableStart = UInt64(offset)
                let tableEnd = tableStart + UInt64(count) * UInt64(width)
                guard start >= tableEnd || tableStart >= end else { throw DeviceError.readFailed }
            }
        }

        // Chunk index → geometry. Each chunk's first point is its anchor (in the
        // ChunkMeta, not the body); the seam anchor of chunks after the first
        // duplicates the previous chunk's last point, so it isn't re-appended.
        var points: [RoutePoint] = []
        for k in 0..<chunkCount {
            let meta = indexOffset + k * chunkMetaLength
            var lon = try reader.i32(at: meta + 16)
            var lat = try reader.i32(at: meta + 20)
            var elevation = try reader.i16(at: meta + 24)
            let pointCount = Int(try reader.u16(at: meta + 26))
            let byteOffset = Int(try reader.u32(at: meta + 36))
            if k == 0 { points.append(routePoint(lon: lon, lat: lat, elevation: elevation)) }
            for r in 0..<max(0, pointCount - 1) {
                let record = byteOffset + r * 7
                lon &+= Int32(try reader.i16(at: record))
                lat &+= Int32(try reader.i16(at: record + 2))
                elevation = try reader.i16(at: record + 4)
                let surface = try reader.u8(at: record + 6)
                guard surface <= 15 else { throw DeviceError.readFailed }
                points.append(routePoint(lon: lon, lat: lat, elevation: elevation, surface: surface))
            }
        }

        var waypoints: [Waypoint] = []
        for k in 0..<waypointCount {
            let base = waypointOffset + k * waypointLength
            let distanceAlong = try reader.u32(at: base)
            let lon = try reader.i32(at: base + 4)
            let lat = try reader.i32(at: base + 8)
                // An unknown category byte reads as generic, never as a decode failure.
            let category = WaypointCategory(wireID: try reader.u8(at: base + 14))
            let nameLength = min(Int(try reader.u8(at: base + 15)), waypointNameCap)
            let lateralOffset = try reader.i16(at: base + 16)
            let name = String(
                decoding: try reader.bytes(at: base + waypointNameOffset, count: nameLength), as: UTF8.self
            )
            let provenanceFlag = try reader.u16(at: base + 78)
            let provenance: WaypointProvenance?
            if provenanceFlag == 1 {
                guard try reader.u64(at: base + 60) > 0, try reader.u64(at: base + 68) > 0 else { throw DeviceError.readFailed }
                provenance = WaypointProvenance(store: try reader.bytes(at: base + 44, count: 16), object: try reader.u64(at: base + 60), revision: try reader.u64(at: base + 68), ordinal: try reader.u16(at: base + 76))
            } else {
                guard provenanceFlag == 0, try reader.bytes(at: base + 44, count: 36).allSatisfy({ $0 == 0 }) else { throw DeviceError.readFailed }
                provenance = nil
            }
            waypoints.append(Waypoint(
                index: k, name: name, distanceAlongMeters: Double(distanceAlong),
                coordinate: Coordinate(latitude: fromMicrodegrees(lat), longitude: fromMicrodegrees(lon)),
                category: category, lateralOffsetMeters: Double(lateralOffset), provenance: provenance
            ))
        }

        return Decoded(
            name: name, version: version, bikeType: bikeType, storedPointCount: storedPointCount,
            totalDistanceMeters: totalDistance, totalAscentMeters: totalAscent,
            totalDescentMeters: totalDescent, minElevationMeters: minElevation,
            maxElevationMeters: maxElevation, start: start, points: points, waypoints: waypoints, unresolvedAvoidance: flags & 1 != 0,
            attributionMap: flags & 4 != 0 ? mapBytes : nil, visitDescriptor: descriptor
        )
    }

    // MARK: Coordinate + rounding helpers

    private static func toMicrodegrees(_ degrees: Double) -> Int32 {
        Int32(clamping: Int64((degrees * 1_000_000).rounded()))
    }

    private static func fromMicrodegrees(_ microdegrees: Int32) -> Double {
        Double(microdegrees) / 1_000_000
    }

    private static func roundToInt16(_ meters: Double) -> Int16 {
        Int16(max(Double(Int16.min) + 1, min(Double(Int16.max), meters.rounded())))
    }

    private static func routePoint(lon: Int32, lat: Int32, elevation: Int16, surface: UInt8 = 0) -> RoutePoint {
        RoutePoint(
            coordinate: Coordinate(latitude: fromMicrodegrees(lat), longitude: fromMicrodegrees(lon)),
            elevationMeters: elevation == Int16.min ? nil : Double(elevation), surface: surface & 7, elevationIncomplete: surface & 8 != 0
        )
    }

}

// MARK: - Little-endian byte plumbing

/// A bounds-checked little-endian view for absolute-offset reads over untrusted device bytes:
/// every under-run is a ``DeviceError/readFailed``, never a crash. OBCR reaches every field by
/// explicit offset, so this reads by offset and not with a cursor.
private struct ByteView {
    private let data: Data
    private let base: Data.Index

    init(_ data: Data) {
        self.data = data
        self.base = data.startIndex
    }

    func bytes(at offset: Int, count: Int) throws -> Data {
        guard offset >= 0, count >= 0, offset + count <= data.count else { throw DeviceError.readFailed }
        return data[(base + offset)..<(base + offset + count)]
    }

    func u8(at offset: Int) throws -> UInt8 { try bytes(at: offset, count: 1).first! }
    func u16(at offset: Int) throws -> UInt16 {
        let b = try bytes(at: offset, count: 2); let i = b.startIndex
        return UInt16(b[i]) | (UInt16(b[i + 1]) << 8)
    }
    func u32(at offset: Int) throws -> UInt32 {
        let b = try bytes(at: offset, count: 4); let i = b.startIndex
        return UInt32(b[i]) | (UInt32(b[i + 1]) << 8) | (UInt32(b[i + 2]) << 16) | (UInt32(b[i + 3]) << 24)
    }
    func u64(at offset: Int) throws -> UInt64 { UInt64(try u32(at: offset)) | (UInt64(try u32(at: offset + 4)) << 32) }
    func i16(at offset: Int) throws -> Int16 { Int16(bitPattern: try u16(at: offset)) }
    func i32(at offset: Int) throws -> Int32 { Int32(bitPattern: try u32(at: offset)) }
}
