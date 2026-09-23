#if os(iOS)
import MapKit
import OBCDomain
import UIKit

/// The share image's map: an Apple Maps snapshot with the track drawn on top. `ImageRenderer`
/// cannot draw a live `Map`, so the share image uses this still.
enum ShareMapSnapshot {
    /// A light-mode snapshot of `size` points at 3x that frames the track, or nil when there is
    /// no network or no track. The snapshot carries the Apple Maps attribution, so the card must
    /// show the image uncropped.
    static func image(for coordinates: [Coordinate], size: CGSize) async -> UIImage? {
        guard coordinates.count > 1 else { return nil }
        let options = MKMapSnapshotter.Options()
        options.region = MapGeometry.boundingRegion(for: coordinates, pad: 1.25)
        options.size = size
        options.scale = 3
        options.pointOfInterestFilter = .excludingAll
        options.traitCollection = UITraitCollection(userInterfaceStyle: .light)
        guard let snapshot = try? await MKMapSnapshotter(options: options).start() else { return nil }
        return drawTrack(coordinates, on: snapshot)
    }

    /// The same halo, stroke and end dots as the live map's `TrackMapContent`.
    private static func drawTrack(_ coordinates: [Coordinate], on snapshot: MKMapSnapshotter.Snapshot) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = snapshot.image.scale
        return UIGraphicsImageRenderer(size: snapshot.image.size, format: format).image { context in
            snapshot.image.draw(at: .zero)
            let points = coordinates.map { snapshot.point(for: MapGeometry.clLocation($0)) }
            let path = UIBezierPath()
            path.move(to: points[0])
            for point in points.dropFirst() { path.addLine(to: point) }
            path.lineCapStyle = .round
            path.lineJoinStyle = .round
            path.lineWidth = 7
            UIColor(OBCTheme.trackHalo).setStroke()
            path.stroke()
            path.lineWidth = 3.4
            UIColor(OBCTheme.trackStroke).setStroke()
            path.stroke()
            dot(at: points[0], fill: UIColor(OBCTheme.trackStart), in: context.cgContext)
            dot(at: points[points.count - 1], fill: UIColor(OBCTheme.trackEnd), in: context.cgContext)
        }
    }

    private static func dot(at center: CGPoint, fill: UIColor, in context: CGContext) {
        let rect = CGRect(x: center.x - 6, y: center.y - 6, width: 12, height: 12)
        UIColor(OBCTheme.panel).setFill()
        context.fillEllipse(in: rect)
        fill.setFill()
        context.fillEllipse(in: rect.insetBy(dx: 2.5, dy: 2.5))
    }
}
#endif
