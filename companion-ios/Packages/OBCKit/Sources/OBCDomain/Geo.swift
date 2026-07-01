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
}

/// A **normalized** polyline for the `GPSTrackPreview` component (B11) to draw —
/// no basemap, ever (epic non-negotiable). Points live in the unit square, already
/// projected + aspect-measured, so the preview view is a dumb `Path` renderer.
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
    /// width ÷ height of the source bounding box, for aspect-correct letterboxing
    /// (1 when there's nothing to draw or the track is a point).
    public let aspectRatio: Double

    public init(points: [Point], aspectRatio: Double) {
        self.points = points
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
        guard coordinates.count > 1 else { return TrackPreview(points: [Point(x: 0.5, y: 0.5)], aspectRatio: 1) }

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
        return TrackPreview(points: points, aspectRatio: aspect)
    }
}
