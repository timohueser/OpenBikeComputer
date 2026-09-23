import Foundation
import OBCDomain

/// OBCR v5 encoder and decoder. Stored geometry owns the route totals. Missing elevation and
/// incoming graph validity stay explicit. The exact bytes are in specs/OBCR_Spec.md, and shared
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
    /// Points per chunk incl. the shared anchor; the resident device index is
    /// bounded by chunk count, not point count.
    static let maxPointsPerChunk = 256
    /// Largest stored per-vertex delta, in microdegrees. A longer segment is densified with
    /// interpolated vertices, so a stored delta never wraps int16.
    static let maxSegmentMicrodegrees = 30_000
    /// Decimation tolerance: drop a vertex within this perpendicular distance (m)
    /// of the chord its neighbours span.
    static let decimationEpsilonMeters = 1.0
    /// Force a kept vertex at least this often (m) so a long near-straight run
    /// keeps shape fidelity at a real point.
    static let maxSpanMeters = 1200.0
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
        let header = ByteView(encode(points: points, waypoints: [], name: "", bikeType: .road))
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
        let nameBytes = truncatedUTF8(name, maxBytes: nameCap)
        var out = payload
        let base = out.startIndex
        out.resetBytes(in: (base + 64)..<(base + 64 + nameCap))
        out[base + 6] = UInt8(nameBytes.count)
        out.replaceSubrange((base + 64)..<(base + 64 + nameBytes.count), with: nameBytes)
        return out
    }

    /// Encode geometry and waypoints into an OBCR v5 file. `waypoints` are stored verbatim,
    /// already placed along the route; `points` carry the geometry and drive the header stats.
    /// Empty `points` yields empty `Data`: there is no valid zero-geometry OBCR.
    public static func encode(points: [RoutePoint], waypoints: [Waypoint], name: String, bikeType: BikeType) -> Data {
        guard !points.isEmpty, points.allSatisfy({ $0.coordinate.isValidGeographic }) else { return Data() }

        // One pass over every raw point: exact stats, distance plus dead-banded ascent and
        // descent, and a per-point candidate carrying the cumulative distance and ascent a kept
        // vertex records in its ChunkMeta.
        var candidates: [Candidate] = []
        candidates.reserveCapacity(points.count)
        var cumulativeDistance = 0.0
        var cumulativeAscent = 0.0
        var cumulativeDescent = 0.0
        var confirmedElevation: Double?
        var minElevation = Int16.max
        var maxElevation = Int16.min
        var previous: Coordinate?
        var bbox: BoundingBox?
        for point in points {
            let coordinate = point.coordinate
            if let previous { cumulativeDistance += previous.routeDistance(to: coordinate) }
            previous = coordinate

            if let elevation = point.elevationMeters {
                let rounded = roundToInt16(elevation)
                minElevation = min(minElevation, rounded)
                maxElevation = max(maxElevation, rounded)
                // Dead-banded like `RouteStats.compute`: climb and descent accrue only once the
                // track has moved past the hysteresis band.
                if let confirmed = confirmedElevation {
                    if elevation >= confirmed + RouteStats.climbHysteresisMeters {
                        cumulativeAscent += elevation - confirmed
                        confirmedElevation = elevation
                    } else if elevation <= confirmed - RouteStats.climbHysteresisMeters {
                        cumulativeDescent += confirmed - elevation
                        confirmedElevation = elevation
                    }
                } else {
                    confirmedElevation = elevation
                }
            } else { confirmedElevation = nil }

            let lon = toMicrodegrees(coordinate.longitude)
            let lat = toMicrodegrees(coordinate.latitude)
            bbox = bbox?.extended(lon: lon, lat: lat) ?? BoundingBox(lon: lon, lat: lat)
            candidates.append(Candidate(
                lon: lon, lat: lat, elevation: point.elevationMeters.map(roundToInt16) ?? Int16.min,
                surface: point.surface | (point.elevationIncomplete ? 8 : 0),
                cumulativeDistance: UInt32(cumulativeDistance),
                cumulativeAscent: UInt32(cumulativeAscent.rounded())
            ))
        }
        if minElevation > maxElevation { minElevation = 0; maxElevation = 0 }

        // Decimate (1-step-lookahead perpendicular distance + max span) into
        // seam-sharing chunks; densification keeps every stored delta in int16 range.
        var encoder = ChunkEncoder(waypoints: waypoints)
        var lastKept: Candidate?
        var pending: Candidate?
        var storedPointCount: UInt32 = 0
        for candidate in candidates {
            switch (lastKept, pending) {
            case (nil, _):
                storedPointCount += encoder.emitDensified(previous: nil, candidate)
                lastKept = candidate
            case (.some, nil):
                pending = candidate
            case (.some(let keep), .some(let mid)):
                let perpendicular = perpendicularDistanceMeters(mid, from: keep, to: candidate)
                let span = Double(candidate.cumulativeDistance - keep.cumulativeDistance)
                if perpendicular > decimationEpsilonMeters || span > maxSpanMeters || keep.elevation != mid.elevation || mid.elevation != candidate.elevation || mid.surface != candidate.surface || reverses(keep, mid, candidate) {
                    storedPointCount += encoder.emitDensified(previous: keep, mid)
                    lastKept = mid
                }
                pending = candidate
            }
        }
        if let pending {  // the final point is always kept
            storedPointCount += encoder.emitDensified(previous: lastKept, pending)
        }
        encoder.finish()

        let box = bbox ?? BoundingBox(lon: 0, lat: 0)
        let start = candidates[0]

        // Physical layout: header, chunk bodies, index, waypoints. Offsets are
        // known once the bodies are sized, so build the sections then the header.
        let dataOffset = headerLength
        let indexData = encoder.encodeIndex()
        let indexOffset = dataOffset + encoder.bodies.count
        let sortedWaypoints = waypoints.map { waypoint in
            let mapped = encoder.waypointDistances[waypoint.index] ?? min(waypoint.distanceAlongMeters, encoder.distance)
            return Waypoint(index: waypoint.index, name: waypoint.name, note: waypoint.note,
                distanceAlongMeters: mapped, coordinate: waypoint.coordinate, category: waypoint.category,
                lateralOffsetMeters: waypoint.lateralOffsetMeters, provenance: waypoint.provenance)
        }.sorted { $0.distanceAlongMeters < $1.distanceAlongMeters }
        let waypointData = encodeWaypoints(sortedWaypoints)
        let waypointOffset = waypointData.isEmpty ? 0 : indexOffset + indexData.count

        var header = Data(count: headerLength)
        header.replaceSubrange(0..<4, with: magic)
        header[4] = version
        header[5] = encoder.hasElevation ? 2 : 0
        let nameBytes = truncatedUTF8(name, maxBytes: nameCap)
        header[6] = UInt8(nameBytes.count)
        header[7] = bikeType.rawValue
        header.putI32(box.minLon, at: 8)
        header.putI32(box.minLat, at: 12)
        header.putI32(box.maxLon, at: 16)
        header.putI32(box.maxLat, at: 20)
        header.putI32(start.lon, at: 24)
        header.putI32(start.lat, at: 28)
        header.putU32(storedPointCount, at: 32)
        header.putU32(UInt32(encoder.distance), at: 36)
        header.putU32(UInt32(encoder.ascent), at: 40)
        header.putU32(UInt32(encoder.descent), at: 44)
        header.putI16(minElevation, at: 48)
        header.putI16(maxElevation, at: 50)
        header.putU32(UInt32(encoder.metas.count), at: 52)
        header.putU32(UInt32(indexOffset), at: 56)
        header.putU32(UInt32(dataOffset), at: 60)
        header.replaceSubrange(64..<(64 + nameBytes.count), with: nameBytes)
        header.putU32(UInt32(waypointOffset), at: 112)
        header.putU16(UInt16(sortedWaypoints.count), at: 116)

        var file = header
        file.append(encoder.bodies)
        file.append(indexData)
        file.append(waypointData)
        return file
    }

    /// The waypoint records, 80 bytes each, in the caller's already distance-sorted order. The
    /// lateral offset is stored saturating, so a waypoint further off route than `Int16` metres
    /// reads as "very far to that side", never as the opposite one.
    private static func encodeWaypoints(_ waypoints: [Waypoint]) -> Data {
        guard !waypoints.isEmpty else { return Data() }
        var data = Data(capacity: waypoints.count * waypointLength)
        for waypoint in waypoints.prefix(Int(UInt16.max)) {
            var record = Data(count: waypointLength)
            record.putU32(UInt32(clamping: Int64(waypoint.distanceAlongMeters.rounded(.down))), at: 0)
            record.putI32(toMicrodegrees(waypoint.coordinate.longitude), at: 4)
            record.putI32(toMicrodegrees(waypoint.coordinate.latitude), at: 8)
            record.putI16(waypointElevationUnknown, at: 12)  // Waypoint carries no elevation
            record[14] = WaypointCategory.wireID(waypoint.category)
            let nameBytes = truncatedUTF8(waypoint.name, maxBytes: waypointNameCap)
            record[15] = UInt8(nameBytes.count)
            record.putI16(lateralOffsetInt16(waypoint.lateralOffsetMeters), at: 16)  // [18..20] reserved
            record.replaceSubrange(waypointNameOffset..<(waypointNameOffset + nameBytes.count), with: nameBytes)
            if let provenance = waypoint.provenance, provenance.store.count == 16 {
                record.replaceSubrange(44..<60, with: provenance.store)
                record.putU64(provenance.object, at: 60)
                record.putU64(provenance.revision, at: 68)
                record.putU16(provenance.ordinal, at: 76)
                record.putU16(1, at: 78)
            }
            data.append(record)
        }
        return data
    }

    /// A signed lateral offset in whole metres, saturating at plus or minus `Int16.max`. The
    /// magnitude is clamped and then signed, exactly as the firmware converter does, so a waypoint
    /// dropped 40 km off route never wraps to the other side. A non-finite offset stores as 0.
    private static func lateralOffsetInt16(_ meters: Double) -> Int16 {
        guard meters.isFinite else { return 0 }
        let magnitude = min(abs(meters).rounded(), Double(Int16.max))
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
        guard flags & ~15 == 0, try reader.u8(at: 119) == 0,
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
        Int16(clamping: max(Int64(Int16.min) + 1, Int64(meters.rounded())))
    }

    private static func routePoint(lon: Int32, lat: Int32, elevation: Int16, surface: UInt8 = 0) -> RoutePoint {
        RoutePoint(
            coordinate: Coordinate(latitude: fromMicrodegrees(lat), longitude: fromMicrodegrees(lon)),
            elevationMeters: elevation == Int16.min ? nil : Double(elevation), surface: surface & 7, elevationIncomplete: surface & 8 != 0
        )
    }

    private static func groundDistance(_ a: Candidate, _ b: Candidate) -> Double {
        let cosLat = cos((Float(a.lat) / 1_000_000) * (Float.pi / 180))
        let x = Float(b.lon - a.lon) * 0.000001 * 111_320 * cosLat
        let y = Float(b.lat - a.lat) * 0.000001 * 111_320
        return Double((x*x + y*y).squareRoot())
    }

    private static func reverses(_ a: Candidate, _ b: Candidate, _ c: Candidate) -> Bool {
        let cosLat = cos(Double(a.lat) * .pi / 180_000_000)
        let ux = Double(b.lon-a.lon) * cosLat, uy = Double(b.lat-a.lat)
        let vx = Double(c.lon-b.lon) * cosLat, vy = Double(c.lat-b.lat)
        return ux*vx + uy*vy < 0
    }

    /// UTF-8 bytes of `string`, truncated to at most `maxBytes` on a character
    /// boundary (never splitting a multi-byte scalar).
    private static func truncatedUTF8(_ string: String, maxBytes: Int) -> Data {
        var bytes = Data()
        for character in string {
            let encoded = Array(String(character).utf8)
            if bytes.count + encoded.count > maxBytes { break }
            bytes.append(contentsOf: encoded)
        }
        return bytes
    }

    /// Perpendicular distance in metres from `point` to the infinite chord `from` to `to`, in a
    /// local-equirectangular metric. Accurate over a route's short segments, and the decimator's
    /// straight-chord test.
    private static func perpendicularDistanceMeters(
        _ point: Candidate, from: Candidate, to: Candidate
    ) -> Double {
        let metersPerDegree = 111_320.0
        let cosLat = Foundation.cos(Double(from.lat) / 1_000_000 * .pi / 180)
        func delta(_ a: Candidate, _ b: Candidate) -> (x: Double, y: Double) {
            (Double(b.lon - a.lon) / 1_000_000 * metersPerDegree * cosLat,
             Double(b.lat - a.lat) / 1_000_000 * metersPerDegree)
        }
        let (cx, cy) = delta(from, to)
        let (px, py) = delta(from, point)
        let length2 = cx * cx + cy * cy
        if length2 <= 1e-9 { return (px * px + py * py).squareRoot() }
        return abs(cx * py - cy * px) / length2.squareRoot()
    }
}

// MARK: - Encoder internals

/// A kept (or interpolated) route vertex plus the cumulative stats a ChunkMeta
/// records for its anchor. Coordinates in microdegrees.
private struct RouteObjectCandidate {
    var lon: Int32
    var lat: Int32
    var elevation: Int16
    var surface: UInt8
    var cumulativeDistance: UInt32
    var cumulativeAscent: UInt32
}

private extension RouteObjectCodec {
    typealias Candidate = RouteObjectCandidate

    struct BoundingBox {
        var minLon: Int32
        var minLat: Int32
        var maxLon: Int32
        var maxLat: Int32

        init(lon: Int32, lat: Int32) {
            minLon = lon; maxLon = lon; minLat = lat; maxLat = lat
        }

        func extended(lon: Int32, lat: Int32) -> BoundingBox {
            var box = self
            box.minLon = min(box.minLon, lon); box.maxLon = max(box.maxLon, lon)
            box.minLat = min(box.minLat, lat); box.maxLat = max(box.maxLat, lat)
            return box
        }
    }

    /// Accumulates kept vertices into seam-sharing chunks of at most ``maxPointsPerChunk``,
    /// streaming each finished chunk's body into `bodies` and its ``ChunkMeta`` into `metas`.
    struct ChunkEncoder {
        var waypoints: [Waypoint] = []
        init(waypoints: [Waypoint]) { self.waypoints = waypoints }
        var waypointDistances: [Int: Double] = [:]
        var bodies = Data()
        var metas: [ChunkMeta] = []
        private var current: [Candidate] = []
        private var dataPosition = RouteObjectCodec.headerLength
        private var chunkStartDistance: UInt32 = 0
        private var chunkStartAscent: UInt32 = 0
        var distance = 0.0
        var ascent = 0.0
        var descent = 0.0
        var hasElevation = false
        private var previous: Candidate?
        private var reference: Double?

        /// Emit `candidate`, first inserting linearly-interpolated vertices so no
        /// stored delta exceeds int16 range. Returns the number of vertices emitted
        /// (intermediates + the candidate) for the header's Point Count.
        mutating func emitDensified(previous: Candidate?, _ candidate: Candidate) -> UInt32 {
            guard let previous else { emit(candidate); return 1 }
            let dLon = Int64(candidate.lon) - Int64(previous.lon)
            let dLat = Int64(candidate.lat) - Int64(previous.lat)
            let maxDelta = max(abs(dLon), abs(dLat))
            var emitted: UInt32 = 0
            if maxDelta > Int64(RouteObjectCodec.maxSegmentMicrodegrees) {
                let steps = maxDelta / Int64(RouteObjectCodec.maxSegmentMicrodegrees) + 1
                for step in 1..<steps {
                    emit(interpolate(previous, candidate, Double(step) / Double(steps)))
                    emitted += 1
                }
            }
            emit(candidate)
            return emitted + 1
        }

        private mutating func emit(_ candidate: Candidate) {
            let before = distance
            let first = previous == nil
            if let previous {
                distance += RouteObjectCodec.groundDistance(previous, candidate)
            }
            for waypoint in waypoints where waypointDistances[waypoint.index] == nil && waypoint.distanceAlongMeters <= Double(candidate.cumulativeDistance) {
                let rawStart = Double(previous?.cumulativeDistance ?? 0)
                let rawLength = Double(candidate.cumulativeDistance) - rawStart
                let fraction = rawLength > 0 ? max(0, min(1, (waypoint.distanceAlongMeters - rawStart) / rawLength)) : 0
                waypointDistances[waypoint.index] = (before + (distance - before) * fraction).rounded(.down)
            }
            previous = candidate
            if candidate.surface & 8 != 0 { reference = nil }
            if candidate.elevation == Int16.min { reference = nil } else {
                hasElevation = true
                let elevation = Double(candidate.elevation)
                if !first && UInt32(distance) == UInt32(before) {
                    // A sub-metre span owns no distance cell.
                } else if let last = reference {
                    let delta = elevation - last
                    if delta >= 3 { ascent += delta; reference = elevation }
                    else if delta <= -3 { descent -= delta; reference = elevation }
                } else { reference = elevation }
            }
            if current.isEmpty {
                chunkStartDistance = UInt32(distance)
                chunkStartAscent = UInt32(ascent)
            }
            current.append(candidate)
            if current.count == RouteObjectCodec.maxPointsPerChunk {
                finalize()
                // Reseed the next chunk with this point as the shared seam / anchor.
                chunkStartDistance = UInt32(distance)
                chunkStartAscent = UInt32(ascent)
                current.append(candidate)
            }
        }

        /// Flush the trailing chunk (skipping a lone seam point already stored in
        /// the prior chunk).
        mutating func finish() {
            if current.count >= 2 || (metas.isEmpty && !current.isEmpty) { finalize() }
        }

        private mutating func finalize() {
            let n = current.count
            guard n > 0 else { return }
            let anchor = current[0]
            var box = BoundingBox(lon: anchor.lon, lat: anchor.lat)
            var body = Data(capacity: (n - 1) * 7)
            for i in 1..<n {
                let point = current[i]
                let previous = current[i - 1]
                // Densification guarantees these deltas fit int16.
                body.appendI16(Int16(point.lon - previous.lon))
                body.appendI16(Int16(point.lat - previous.lat))
                body.appendI16(point.elevation)
                body.append(point.surface)
                box = box.extended(lon: point.lon, lat: point.lat)
            }
            metas.append(ChunkMeta(
                minLon: box.minLon, minLat: box.minLat, maxLon: box.maxLon, maxLat: box.maxLat,
                anchorLon: anchor.lon, anchorLat: anchor.lat, anchorElevation: anchor.elevation,
                pointCount: UInt16(n), cumulativeDistance: chunkStartDistance,
                cumulativeAscent: chunkStartAscent, byteOffset: UInt32(dataPosition),
                byteLength: UInt32(body.count)
            ))
            bodies.append(body)
            dataPosition += body.count
            current.removeAll(keepingCapacity: true)
        }

        func encodeIndex() -> Data {
            var data = Data(capacity: metas.count * RouteObjectCodec.chunkMetaLength)
            for meta in metas {
                var record = Data(count: RouteObjectCodec.chunkMetaLength)
                record.putI32(meta.minLon, at: 0)
                record.putI32(meta.minLat, at: 4)
                record.putI32(meta.maxLon, at: 8)
                record.putI32(meta.maxLat, at: 12)
                record.putI32(meta.anchorLon, at: 16)
                record.putI32(meta.anchorLat, at: 20)
                record.putI16(meta.anchorElevation, at: 24)
                record.putU16(meta.pointCount, at: 26)
                record.putU32(meta.cumulativeDistance, at: 28)
                record.putU32(meta.cumulativeAscent, at: 32)
                record.putU32(meta.byteOffset, at: 36)
                record.putU32(meta.byteLength, at: 40)
                data.append(record)
            }
            return data
        }

        private func interpolate(_ a: Candidate, _ b: Candidate, _ t: Double) -> Candidate {
            func lerpI32(_ from: Int32, _ to: Int32) -> Int32 {
                Int32((Double(from) + (Double(to) - Double(from)) * t).rounded())
            }
            func lerpU32(_ from: UInt32, _ to: UInt32) -> UInt32 {
                UInt32((Double(from) + (Double(to) - Double(from)) * t).rounded())
            }
            return Candidate(
                lon: lerpI32(a.lon, b.lon), lat: lerpI32(a.lat, b.lat),
                elevation: a.elevation == Int16.min || b.elevation == Int16.min || b.surface & 8 != 0 ? Int16.min : Int16((Double(a.elevation) + (Double(b.elevation) - Double(a.elevation)) * t).rounded()),
                surface: b.surface,
                cumulativeDistance: lerpU32(a.cumulativeDistance, b.cumulativeDistance),
                cumulativeAscent: lerpU32(a.cumulativeAscent, b.cumulativeAscent)
            )
        }
    }

    /// One chunk's index entry.
    struct ChunkMeta {
        var minLon: Int32
        var minLat: Int32
        var maxLon: Int32
        var maxLat: Int32
        var anchorLon: Int32
        var anchorLat: Int32
        var anchorElevation: Int16
        var pointCount: UInt16
        var cumulativeDistance: UInt32
        var cumulativeAscent: UInt32
        var byteOffset: UInt32
        var byteLength: UInt32
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

private extension Data {
    mutating func appendI16(_ value: Int16) {
        let u = UInt16(bitPattern: value)
        append(UInt8(u & 0xFF)); append(UInt8(u >> 8))
    }

    mutating func putU16(_ value: UInt16, at offset: Int) {
        let i = startIndex + offset
        self[i] = UInt8(value & 0xFF); self[i + 1] = UInt8(value >> 8)
    }

    mutating func putI16(_ value: Int16, at offset: Int) { putU16(UInt16(bitPattern: value), at: offset) }

    mutating func putU32(_ value: UInt32, at offset: Int) {
        let i = startIndex + offset
        self[i] = UInt8(value & 0xFF); self[i + 1] = UInt8((value >> 8) & 0xFF)
        self[i + 2] = UInt8((value >> 16) & 0xFF); self[i + 3] = UInt8((value >> 24) & 0xFF)
    }

    mutating func putU64(_ value: UInt64, at offset: Int) {
        putU32(UInt32(truncatingIfNeeded: value), at: offset)
        putU32(UInt32(value >> 32), at: offset + 4)
    }

    mutating func putI32(_ value: Int32, at offset: Int) { putU32(UInt32(bitPattern: value), at: offset) }
}
