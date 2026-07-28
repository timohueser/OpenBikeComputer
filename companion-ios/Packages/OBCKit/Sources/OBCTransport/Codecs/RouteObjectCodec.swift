import Foundation
import OBCDomain

/// The **route object** codec — the phone-side `ImportedRoute → OBCR v2` encoder
/// (and its reader), the upload sibling of ``RideObjectCodec``. A route object's
/// payload is *exactly the bytes of an OBCR v2 file* (`obc-ble-interface-spec.md`
/// §7.1); the device writes it to SD verbatim and rides it through the same
/// `no_std` `obc-route` reader the firmware runs.
///
/// This is a **hand-written all-Swift** port of the layout in
/// [`OBCR_Spec.md`](../../../../../../specs/OBCR_Spec.md) — the same choice `OBCKit`
/// made for the ride/config codecs. Byte-for-byte parity with the Rust
/// `gpx_to_obcr` is **not** a goal (the firmware only needs *valid* OBCR, and the
/// two producers decimate independently); the geometry math here mirrors the app's
/// own ``RouteStats`` so an uploaded route's stored header totals match exactly
/// what E1 displayed. The `route-plain.obcr` / `route-waypoints.obcr` shared
/// fixtures (`specs/vectors/`, produced by the firmware converter) pin the
/// **reader** so it can't drift from real device output.
///
/// File layout (little-endian throughout, coordinates in microdegrees):
/// ```
/// [Header 128 B][Chunk 0 data]…[Chunk N-1 data][Chunk Index N×44 B][Waypoints M×40 B]
/// ```
/// The header (patched last, once offsets are known) reaches the index and the v2
/// waypoint table by explicit offset; geometry is split into ≤``maxPointsPerChunk``
/// **seam-sharing** chunks (chunk k's last point == chunk k+1's anchor), each an
/// anchor + `int16` deltas. See `OBCR_Spec.md` §§1–5.
public enum RouteObjectCodec {
    // MARK: Format constants (OBCR_Spec.md)

    static let magic = Data("OBCR".utf8)
    static let version: UInt8 = 2
    /// v1 base header; every ride-path field lives here. v2 appends the 16-byte
    /// waypoint extension at offset 112.
    static let headerBaseLength = 112
    static let headerLength = 128
    static let chunkMetaLength = 44
    static let waypointLength = 40
    /// Header route-name cap (matches the device `NAME_CAP`).
    static let nameCap = 48
    /// Waypoint short-name cap.
    static let waypointNameCap = 24
    /// Points per chunk incl. the shared anchor; the resident device index is
    /// bounded by chunk count, not point count.
    static let maxPointsPerChunk = 256
    /// Largest stored per-vertex delta (µdeg). A longer segment is densified with
    /// interpolated vertices so `(x − px) as int16` never wraps — mirrors the
    /// converter's `MAX_SEGMENT_UDEG` / the OBCM packer.
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

    /// Encode an imported route's geometry + waypoints into an OBCR v2 file, named
    /// `name` (truncated to ``nameCap`` on a char boundary). Convenience over the
    /// `points`/`waypoints` form for the "ImportedRoute → payload" call site.
    public static func encode(_ route: ImportedRoute, name: String) -> Data {
        encode(points: route.points, waypoints: route.waypoints, name: name)
    }

    /// The CRC-32 of the payload an upload of this library record would send —
    /// the `OnDeviceState` fingerprint. **The one canonical "payload for a
    /// record" definition**: the record's geometry + waypoints under its display
    /// name, exactly what the detail screen's upload blob encodes; keeping both
    /// on this function is what makes "up to date" mean byte-identical.
    public static func payloadCRC(for record: PlannedRouteRecord) -> UInt32 {
        CRC32.checksum(encode(points: record.route.points, waypoints: record.route.waypoints, name: record.summary.name))
    }

    /// Encode geometry + waypoints into an OBCR v2 file. `waypoints` are stored
    /// verbatim (already placed along the route by `WaypointPlacement`); `points`
    /// carry the geometry and drive the exact header stats.
    ///
    /// An empty `points` yields an empty `Data` — there is no valid zero-geometry
    /// OBCR, and the upload path only reaches here with a decoded route in hand.
    public static func encode(points: [RoutePoint], waypoints: [Waypoint], name: String) -> Data {
        guard !points.isEmpty else { return Data() }

        // One pass over every raw point: exact stats (distance + dead-banded
        // ascent/descent, mirroring RouteStats so the header matches the E1 display)
        // and a per-point candidate carrying the cumulative distance/ascent a kept
        // vertex records in its ChunkMeta.
        var candidates: [Candidate] = []
        candidates.reserveCapacity(points.count)
        var cumulativeDistance = 0.0
        var cumulativeAscent = 0.0
        var cumulativeDescent = 0.0
        var confirmedElevation: Double?
        var lastElevation = 0.0
        var minElevation = Int16.max
        var maxElevation = Int16.min
        var previous: Coordinate?
        var bbox: BoundingBox?
        for point in points {
            let coordinate = point.coordinate
            if let previous { cumulativeDistance += previous.distance(to: coordinate) }
            previous = coordinate

            if let elevation = point.elevationMeters {
                lastElevation = elevation
                let rounded = roundToInt16(elevation)
                minElevation = min(minElevation, rounded)
                maxElevation = max(maxElevation, rounded)
                // Dead-banded like RouteStats.compute: climb/descent only accrue
                // once the track has moved past the hysteresis band.
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
            }

            let lon = toMicrodegrees(coordinate.longitude)
            let lat = toMicrodegrees(coordinate.latitude)
            bbox = bbox?.extended(lon: lon, lat: lat) ?? BoundingBox(lon: lon, lat: lat)
            candidates.append(Candidate(
                lon: lon, lat: lat, elevation: roundToInt16(lastElevation),
                cumulativeDistance: UInt32(cumulativeDistance.rounded()),
                cumulativeAscent: UInt32(cumulativeAscent.rounded())
            ))
        }
        if minElevation > maxElevation { minElevation = 0; maxElevation = 0 }

        // Decimate (1-step-lookahead perpendicular distance + max span) into
        // seam-sharing chunks; densification keeps every stored delta in int16 range.
        var encoder = ChunkEncoder()
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
                if perpendicular > decimationEpsilonMeters || span > maxSpanMeters {
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
        let sortedWaypoints = waypoints.sorted { $0.distanceAlongMeters < $1.distanceAlongMeters }
        let waypointData = encodeWaypoints(sortedWaypoints)
        let waypointOffset = waypointData.isEmpty ? 0 : indexOffset + indexData.count

        var header = Data(count: headerLength)
        header.replaceSubrange(0..<4, with: magic)
        header[4] = version  // [5] flags, [7] reserved already 0
        let nameBytes = truncatedUTF8(name, maxBytes: nameCap)
        header[6] = UInt8(nameBytes.count)
        header.putI32(box.minLon, at: 8)
        header.putI32(box.minLat, at: 12)
        header.putI32(box.maxLon, at: 16)
        header.putI32(box.maxLat, at: 20)
        header.putI32(start.lon, at: 24)
        header.putI32(start.lat, at: 28)
        header.putU32(storedPointCount, at: 32)
        header.putU32(UInt32(cumulativeDistance.rounded()), at: 36)
        header.putU32(UInt32(cumulativeAscent.rounded()), at: 40)
        header.putU32(UInt32(cumulativeDescent.rounded()), at: 44)
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

    private static func encodeWaypoints(_ waypoints: [Waypoint]) -> Data {
        guard !waypoints.isEmpty else { return Data() }
        var data = Data(capacity: waypoints.count * waypointLength)
        for waypoint in waypoints.prefix(Int(UInt16.max)) {
            var record = Data(count: waypointLength)
            record.putU32(UInt32(clamping: Int64(waypoint.distanceAlongMeters.rounded())), at: 0)
            record.putI32(toMicrodegrees(waypoint.coordinate.longitude), at: 4)
            record.putI32(toMicrodegrees(waypoint.coordinate.latitude), at: 8)
            record.putI16(waypointElevationUnknown, at: 12)  // Waypoint carries no elevation
            record[14] = 0  // type: generic — the app doesn't map <sym>/course-point types yet
            let nameBytes = truncatedUTF8(waypoint.name, maxBytes: waypointNameCap)
            record[15] = UInt8(nameBytes.count)
            record.replaceSubrange(16..<(16 + nameBytes.count), with: nameBytes)
            data.append(record)
        }
        return data
    }

    // MARK: Decode

    /// The parsed contents of an OBCR file — the header stats (exact, from the
    /// producer's raw-point pass), the deduped geometry (seams counted once), and
    /// the waypoints. Feeds the BLE `routeDetail` read and pins the reader against
    /// the shared firmware fixtures.
    public struct Decoded: Equatable, Sendable {
        public var name: String
        public var version: UInt8
        /// Header `Point Count` (distinct stored points; may exceed `points.count`
        /// only if the file's stored count disagrees with its geometry — it won't
        /// for a well-formed file).
        public var storedPointCount: UInt32
        public var totalDistanceMeters: UInt32
        public var totalAscentMeters: UInt32
        public var totalDescentMeters: UInt32
        public var minElevationMeters: Int16
        public var maxElevationMeters: Int16
        /// First route point (camera centering).
        public var start: Coordinate
        /// The decoded polyline, seams deduplicated — every stored vertex once.
        public var points: [RoutePoint]
        public var waypoints: [Waypoint]
    }

    /// Decode an OBCR v1/v2 file. Every section is reached by an explicit offset
    /// and bounds-checked — malformed device bytes throw ``DeviceError/readFailed``,
    /// never trap.
    public static func decode(_ data: Data) throws -> Decoded {
        let reader = ByteView(data)
        guard try reader.bytes(at: 0, count: 4) == magic else { throw DeviceError.readFailed }
        let version = try reader.u8(at: 4)
        guard version == 1 || version == 2 else { throw DeviceError.readFailed }
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
        if version >= 2, data.count >= headerLength {
            waypointOffset = Int(try reader.u32(at: 112))
            waypointCount = Int(try reader.u16(at: 116))
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
                let record = byteOffset + r * 6
                lon &+= Int32(try reader.i16(at: record))
                lat &+= Int32(try reader.i16(at: record + 2))
                elevation = try reader.i16(at: record + 4)
                points.append(routePoint(lon: lon, lat: lat, elevation: elevation))
            }
        }

        var waypoints: [Waypoint] = []
        for k in 0..<waypointCount {
            let base = waypointOffset + k * waypointLength
            let distanceAlong = try reader.u32(at: base)
            let lon = try reader.i32(at: base + 4)
            let lat = try reader.i32(at: base + 8)
            let nameLength = min(Int(try reader.u8(at: base + 15)), waypointNameCap)
            let name = String(decoding: try reader.bytes(at: base + 16, count: nameLength), as: UTF8.self)
            waypoints.append(Waypoint(
                index: k, name: name, distanceAlongMeters: Double(distanceAlong),
                coordinate: Coordinate(latitude: fromMicrodegrees(lat), longitude: fromMicrodegrees(lon))
            ))
        }

        return Decoded(
            name: name, version: version, storedPointCount: storedPointCount,
            totalDistanceMeters: totalDistance, totalAscentMeters: totalAscent,
            totalDescentMeters: totalDescent, minElevationMeters: minElevation,
            maxElevationMeters: maxElevation, start: start, points: points, waypoints: waypoints
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
        Int16(clamping: Int64(meters.rounded()))
    }

    private static func routePoint(lon: Int32, lat: Int32, elevation: Int16) -> RoutePoint {
        RoutePoint(
            coordinate: Coordinate(latitude: fromMicrodegrees(lat), longitude: fromMicrodegrees(lon)),
            elevationMeters: Double(elevation)
        )
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

    /// Perpendicular distance (m) from `point` to the infinite chord `from → to`,
    /// in a local-equirectangular metric (east scaled by cos(lat)) — accurate over
    /// a route's short segments, the decimator's straight-chord test.
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

    /// Accumulates kept vertices into ≤``maxPointsPerChunk`` seam-sharing chunks,
    /// streaming each finished chunk's body into `bodies` and its ``ChunkMeta`` into
    /// `metas` (byte offsets absolute from the file start = ``headerLength`` + body).
    struct ChunkEncoder {
        var bodies = Data()
        var metas: [ChunkMeta] = []
        private var current: [Candidate] = []
        private var dataPosition = RouteObjectCodec.headerLength
        private var chunkStartDistance: UInt32 = 0
        private var chunkStartAscent: UInt32 = 0

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
            if current.isEmpty {
                chunkStartDistance = candidate.cumulativeDistance
                chunkStartAscent = candidate.cumulativeAscent
            }
            current.append(candidate)
            if current.count == RouteObjectCodec.maxPointsPerChunk {
                finalize()
                // Reseed the next chunk with this point as the shared seam / anchor.
                chunkStartDistance = candidate.cumulativeDistance
                chunkStartAscent = candidate.cumulativeAscent
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
            var body = Data(capacity: (n - 1) * 6)
            for i in 1..<n {
                let point = current[i]
                let previous = current[i - 1]
                // Densification guarantees these deltas fit int16.
                body.appendI16(Int16(point.lon - previous.lon))
                body.appendI16(Int16(point.lat - previous.lat))
                body.appendI16(point.elevation)
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
                elevation: Int16((Double(a.elevation) + (Double(b.elevation) - Double(a.elevation)) * t).rounded()),
                cumulativeDistance: lerpU32(a.cumulativeDistance, b.cumulativeDistance),
                cumulativeAscent: lerpU32(a.cumulativeAscent, b.cumulativeAscent)
            )
        }
    }

    /// One chunk's index entry (`OBCR_Spec.md` §2).
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

/// A bounds-checked little-endian view for absolute-offset reads over untrusted
/// device bytes — every under-run is a ``DeviceError/readFailed``, never a crash.
/// (OBCR reaches every field by explicit offset, so this reads by offset, not a
/// cursor.)
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

    mutating func putI32(_ value: Int32, at offset: Int) { putU32(UInt32(bitPattern: value), at: offset) }
}
