import Foundation

/// A WGS-84 geographic coordinate. A plain value type so it crosses the
/// `DeviceTransport` boundary without dragging in CoreLocation (which `OBCDomain`
/// deliberately avoids — see `companion-ios/CLAUDE.md` → layering).
public struct Coordinate: Hashable, Sendable {
    public let latitude: Double
    public let longitude: Double

    public init(latitude: Double, longitude: Double) {
        self.latitude = latitude
        self.longitude = longitude
    }

    /// Whether both components are finite **and** within WGS-84 range
    /// (lat ∈ [-90, 90], lon ∈ [-180, 180]). `init` stays cheap and
    /// non-failing for the trusted paths (device decode, previews); the file
    /// import edge validates against this so a malformed GPX/TCX throws
    /// `FormatError.malformed` instead of a non-finite coordinate poisoning
    /// `distance()` (→ NaN) and everything downstream of it (#304).
    public var isValidGeographic: Bool {
        latitude.isFinite && longitude.isFinite
            && (-90.0...90.0).contains(latitude)
            && (-180.0...180.0).contains(longitude)
    }

    /// Great-circle distance to `other` in metres (haversine, spherical Earth).
    /// Plenty for route stats and waypoint placement; no CoreLocation.
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

/// A polyline for the `GPSTrackPreview` component (B11) to draw. Carries two
/// parallel representations of the same downsampled track:
///
///   • `points` — the unit-square, aspect-measured projection the **grid
///     fallback** renderer draws directly (a dumb `Path` renderer, no basemap).
///   • `coordinates` — the source WGS-84 lat/lon, so the **MapKit basemap**
///     preview (#294) can draw a real `MapPolyline` and fit a camera to the
///     track's bounds without re-deriving geography.
///
/// The two arrays are the same length and index-aligned (same downsample). The
/// basemap path uses `coordinates`; when it's empty (or the device is offline)
/// the preview degrades to the `points` grid — an intentional fallback, not a
/// bug (see `companion-ios/CLAUDE.md`).
///
/// Produced from route/ride geometry by both the mock fixtures (B1M) and the real
/// decode path (`B1`/`BLEChannel`) — so the projection lives here, in the shared
/// domain layer, not duplicated in each.
public struct TrackPreview: Equatable, Sendable {
    /// A single point in unit space.
    public struct Point: Hashable, Sendable {
        /// 0…1, left → right.
        public let x: Double
        /// 0…1, top → bottom (**y-down**, so it feeds SwiftUI `Path` directly;
        /// north maps to the top).
        public let y: Double

        public init(x: Double, y: Double) {
            self.x = x
            self.y = y
        }
    }

    /// The polyline in unit space. Empty when the source had no geometry.
    public let points: [Point]
    /// The source WGS-84 coordinates for `points`, index-aligned (same
    /// downsample). Empty when unknown (a legacy library file, or a source that
    /// only kept the normalized shape) — the basemap preview then falls back to
    /// the grid.
    public let coordinates: [Coordinate]
    /// width ÷ height of the source bounding box, for aspect-correct letterboxing
    /// (1 when there's nothing to draw or the track is a point).
    public let aspectRatio: Double

    public init(points: [Point], aspectRatio: Double, coordinates: [Coordinate] = []) {
        self.points = points
        self.coordinates = coordinates
        self.aspectRatio = aspectRatio
    }

    /// Empty preview — nothing to draw.
    public static let empty = TrackPreview(points: [], aspectRatio: 1)

    /// Project + normalize a geographic polyline into the unit square, optionally
    /// downsampling to at most `maxPoints` (uniform stride — enough for a thumbnail;
    /// a shape-preserving simplifier is overkill for a preview).
    ///
    /// Projection is equirectangular around the centroid latitude — the longitude
    /// axis is scaled by `cos(lat)` so the aspect ratio looks right at any latitude.
    /// Degenerate tracks (0/1 points, zero-width bbox) collapse to the centre
    /// instead of dividing by zero.
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
        // `sampled` is index-aligned with `points` (both come off the same
        // downsample), so the basemap path can draw the real lat/lon polyline.
        return TrackPreview(points: points, aspectRatio: aspect, coordinates: sampled)
    }
}
