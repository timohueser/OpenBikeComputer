#if os(iOS)
import MapKit
import OBCDomain
import UIKit

/// The share image's map: an Apple Maps snapshot with the line drawn on top. `ImageRenderer`
/// cannot draw a live `Map`, so the share image uses this still.
enum ShareMapSnapshot {
    /// A light-mode snapshot of `size` points at 3x that frames the line, or nil when there is
    /// no network or no line. The snapshot carries the Apple Maps attribution, so the card must
    /// show the image uncropped.
    static func image(for stages: [MultiTrackPreviewView.Stage], size: CGSize) async -> UIImage? {
        let coordinates = stages.flatMap(\.coordinates)
        guard coordinates.count > 1 else { return nil }
        let options = MKMapSnapshotter.Options()
        options.region = MapGeometry.boundingRegion(for: coordinates, pad: 1.25)
        options.size = size
        options.scale = 3
        options.pointOfInterestFilter = .excludingAll
        options.traitCollection = UITraitCollection(userInterfaceStyle: .light)
        guard let snapshot = try? await MKMapSnapshotter(options: options).start() else { return nil }
        return draw(stages, on: snapshot)
    }

    /// The same casing, stroke and end marks as the live map's `TrackMapContent`. A dashed stage
    /// draws thinner and without the casing, as on the trip review's map.
    private static func draw(_ stages: [MultiTrackPreviewView.Stage], on snapshot: MKMapSnapshotter.Snapshot) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = snapshot.image.scale
        return UIGraphicsImageRenderer(size: snapshot.image.size, format: format).image { context in
            snapshot.image.draw(at: .zero)
            for stage in stages where stage.coordinates.count > 1 {
                let points = stage.coordinates.map { snapshot.point(for: MapGeometry.clLocation($0)) }
                let path = UIBezierPath()
                path.move(to: points[0])
                for point in points.dropFirst() { path.addLine(to: point) }
                path.lineCapStyle = .round
                path.lineJoinStyle = .round
                if !stage.dash.isEmpty {
                    path.lineWidth = 2.4
                    path.setLineDash(stage.dash, count: stage.dash.count, phase: 0)
                } else {
                    if stage.cased {
                        path.lineWidth = 7
                        UIColor(OBCTheme.routeCasing).setStroke()
                        path.stroke()
                    }
                    path.lineWidth = 3.4
                }
                UIColor(stage.color).setStroke()
                path.stroke()
            }
            let ends = stages.flatMap(\.coordinates).map { snapshot.point(for: MapGeometry.clLocation($0)) }
            mark({ UIBezierPath(ovalIn: $0) }, at: ends[0], fill: UIColor(OBCTheme.ink))
            mark({ UIBezierPath(roundedRect: $0, cornerRadius: 2) }, at: ends[ends.count - 1], fill: UIColor(OBCTheme.rust))
        }
    }

    /// An end mark ringed in the surface colour: the start's ink dot or the end's rust square.
    private static func mark(_ shape: (CGRect) -> UIBezierPath, at center: CGPoint, fill: UIColor) {
        let rect = CGRect(x: center.x - 6, y: center.y - 6, width: 12, height: 12)
        UIColor(OBCTheme.surface).setFill()
        shape(rect).fill()
        fill.setFill()
        shape(rect.insetBy(dx: 2, dy: 2)).fill()
    }
}
#endif
