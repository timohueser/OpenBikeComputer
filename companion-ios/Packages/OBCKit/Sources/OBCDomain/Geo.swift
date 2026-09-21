import Foundation

/// A WGS-84 geographic coordinate. A plain value type, so it crosses the `DeviceTransport`
/// boundary without dragging in CoreLocation, which `OBCDomain` deliberately avoids.
public struct Coordinate: Hashable, Sendable {
    public let latitude: Double
    public let longitude: Double

    public init(latitude: Double, longitude: Double) {
        self.latitude = latitude
        self.longitude = longitude
    }

    /// Whether both components are finite and within WGS-84 range. `init` stays cheap and
    /// non-failing for the trusted paths, and the file-import edge validates against this, so a
    /// malformed file throws instead of a non-finite coordinate poisoning `distance()`, and
    /// everything downstream of it, with NaN.
    public var isValidGeographic: Bool {
        latitude.isFinite && longitude.isFinite
            && (-90.0...90.0).contains(latitude)
            && (-180.0...180.0).contains(longitude)
    }

    /// Stored route metric: microdegree coordinates, Float segment math, Double accumulation.
    public func routeDistance(to other: Coordinate) -> Double {
        guard isValidGeographic, other.isValidGeographic else { return .infinity }
        let lon = Int32((longitude * 1_000_000).rounded())
        let lat = Int32((latitude * 1_000_000).rounded())
        let otherLon = Int32((other.longitude * 1_000_000).rounded())
        let otherLat = Int32((other.latitude * 1_000_000).rounded())
        let cosLat = cos((Float(lat) / 1_000_000) * (Float.pi / 180))
        let x = Float(otherLon - lon) * 0.000001 * 111_320 * cosLat
        let y = Float(otherLat - lat) * 0.000001 * 111_320
        return Double((x*x + y*y).squareRoot())
    }

    /// Great-circle distance to `other` in metres, spherical Earth. Plenty for route stats and
    /// waypoint placement, and no CoreLocation.
    public func distance(to other: Coordinate) -> Double {
        let earthRadius = 6_371_000.0
        let lat1 = latitude * .pi / 180
        let lat2 = other.latitude * .pi / 180
        let dLat = lat2 - lat1
        let dLon = (other.longitude - longitude) * .pi / 180
        let a = sin(dLat / 2) * sin(dLat / 2) + cos(lat1) * cos(lat2) * sin(dLon / 2) * sin(dLon / 2)
        return 2 * earthRadius * atan2(sqrt(a), sqrt(1 - a))
    }
}

/// A polyline for the track-preview component to draw. It carries two parallel representations of
/// the same downsampled track: `points`, the unit-square, aspect-measured projection the grid
/// fallback renderer draws directly, and `coordinates`, the source lat and lon, so the basemap
/// preview can draw a real polyline and fit a camera to the track's bounds without re-deriving
/// geography.
///
/// The two arrays are the same length and index-aligned. The basemap path uses `coordinates`, and
/// when it is empty, or the device is offline, the preview degrades to the grid. That is an
/// intentional fallback, not a bug.
///
/// Produced from route and ride geometry by both the mock fixtures and the real decode path, so
/// the projection lives here in the shared domain layer rather than in each.
public struct TrackPreview: Equatable, Sendable {
    /// A single point in unit space.
    public struct Point: Hashable, Sendable {
        /// 0…1, left → right.
        public let x: Double
        /// 0 to 1, top to bottom. Y-down, so it feeds a SwiftUI `Path` directly and north maps to
        /// the top.
        public let y: Double

        public init(x: Double, y: Double) {
            self.x = x
            self.y = y
        }
    }

    /// The polyline in unit space. Empty when the source had no geometry.
    public let points: [Point]
    /// The source coordinates for `points`, index-aligned. Empty when unknown, such as an older
    /// library file, and the basemap preview then falls back to the grid.
    public let coordinates: [Coordinate]
    /// Width divided by height of the source bounding box, for aspect-correct letterboxing. It is
    /// 1 when there is nothing to draw, or the track is a point.
    public let aspectRatio: Double

    public init(points: [Point], aspectRatio: Double, coordinates: [Coordinate] = []) {
        self.points = points
        self.coordinates = coordinates
        self.aspectRatio = aspectRatio
    }

    /// Empty preview — nothing to draw.
    public static let empty = TrackPreview(points: [], aspectRatio: 1)

    /// Project and normalize a geographic polyline into the unit square, optionally downsampling
    /// to at most `maxPoints` with a uniform stride, which is enough for a thumbnail.
    ///
    /// The projection is equirectangular around the centroid latitude: the longitude axis is
    /// scaled by the cosine of the latitude, so the aspect ratio looks right at any latitude. A
    /// degenerate track collapses to the centre instead of dividing by zero.
    public static func normalizing(_ coordinates: [Coordinate], maxPoints: Int = 256) -> TrackPreview {
        guard !coordinates.isEmpty else { return .empty }
        guard coordinates.count > 1 else {
            return TrackPreview(points: [Point(x: 0.5, y: 0.5)], aspectRatio: 1, coordinates: coordinates)
        }

        // Uniform-stride downsample, always keeping the last point.
        let sampled: [Coordinate]
        if maxPoints > 1, coordinates.count > maxPoints {
            let stride = Double(coordinates.count - 1) / Double(maxPoints - 1)
            sampled = (0..<maxPoints).map { coordinates[Int((Double($0) * stride).rounded())] }
        } else {
            sampled = coordinates
        }

        // Equirectangular projection around the centroid latitude.
        let meanLat = sampled.reduce(0) { $0 + $1.latitude } / Double(sampled.count)
        let lonScale = Foundation.cos(meanLat * .pi / 180)
        let projected = sampled.map { (x: $0.longitude * lonScale, y: $0.latitude) }

        let xs = projected.map(\.x)
        let ys = projected.map(\.y)
        let minX = xs.min()!, maxX = xs.max()!
        let minY = ys.min()!, maxY = ys.max()!
        let spanX = maxX - minX
        let spanY = maxY - minY
        let aspect = spanY > 0 ? (spanX > 0 ? spanX / spanY : 1) : 1

        let points = projected.map { p -> Point in
            let u = spanX > 0 ? (p.x - minX) / spanX : 0.5
            let v = spanY > 0 ? (p.y - minY) / spanY : 0.5
            return Point(x: u, y: 1 - v)  // flip so north is at the top
        }
        // `sampled` is index-aligned with `points`, because both come off the same downsample, so
        // the basemap path can draw the real polyline.
        return TrackPreview(points: points, aspectRatio: aspect, coordinates: sampled)
    }

    /// Project and normalize several tracks into one shared unit square, so they can be drawn
    /// overlaid and aligned for a trip card's multi-stage preview. Every returned preview shares
    /// the same bounding box and aspect ratio, from one projection around the combined centroid
    /// latitude, so a single fitting transform lays all the stages out in register. Each track is
    /// downsampled independently, and an empty input track maps to `.empty`.
    public static func normalizingShared(
        _ tracks: [[Coordinate]], maxPointsPerTrack: Int = 256
    ) -> [TrackPreview] {
        let sampledTracks = tracks.map { downsampleCoordinates($0, to: maxPointsPerTrack) }
        let all = sampledTracks.flatMap { $0 }
        guard !all.isEmpty else { return tracks.map { _ in .empty } }

        // One equirectangular projection around the combined centroid latitude.
        let meanLat = all.reduce(0) { $0 + $1.latitude } / Double(all.count)
        let lonScale = Foundation.cos(meanLat * .pi / 180)
        func project(_ c: Coordinate) -> (x: Double, y: Double) {
            (c.longitude * lonScale, c.latitude)
        }
        let projected = all.map(project)
        let minX = projected.map(\.x).min()!, maxX = projected.map(\.x).max()!
        let minY = projected.map(\.y).min()!, maxY = projected.map(\.y).max()!
        let spanX = maxX - minX
        let spanY = maxY - minY
        let aspect = spanY > 0 ? (spanX > 0 ? spanX / spanY : 1) : 1

        return sampledTracks.map { track in
            guard !track.isEmpty else { return TrackPreview.empty }
            let points = track.map { c -> Point in
                let p = project(c)
                let u = spanX > 0 ? (p.x - minX) / spanX : 0.5
                let v = spanY > 0 ? (p.y - minY) / spanY : 0.5
                return Point(x: u, y: 1 - v)  // flip so north is at the top
            }
            return TrackPreview(points: points, aspectRatio: aspect, coordinates: track)
        }
    }

    /// Uniform-stride downsample of a coordinate array, always keeping the last point: the same
    /// rule ``normalizing(_:maxPoints:)`` applies inline.
    private static func downsampleCoordinates(
        _ coordinates: [Coordinate], to maxPoints: Int
    ) -> [Coordinate] {
        guard maxPoints > 1, coordinates.count > maxPoints else { return coordinates }
        let stride = Double(coordinates.count - 1) / Double(maxPoints - 1)
        return (0..<maxPoints).map { coordinates[Int((Double($0) * stride).rounded())] }
    }
}
