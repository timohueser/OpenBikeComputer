import SwiftUI
import OBCDomain

/// The basemap-free polyline on gridded parchment that identifies every route and
/// ride in the app. Renders the normalized `TrackPreview` (unit-square points,
/// y-down), letterboxed to the source aspect ratio. Never a basemap.
///
/// The card chrome is on by default; the compact route card turns it off for its
/// flush left cell.
public struct TrackPreviewView: View {
    public enum Style {
        case thumbnail
        case hero

        var dotRadius: CGFloat {
            switch self {
            case .thumbnail: 4.5
            case .hero: 6
            }
        }
    }

    /// An extra dot pinned on the polyline: a unit point plus a label drawn in an
    /// amber marker.
    public struct Marker: Identifiable {
        public let id: Int
        public let point: TrackPreview.Point
        public let label: String

        public init(id: Int, point: TrackPreview.Point, label: String) {
            self.id = id
            self.point = point
            self.label = label
        }
    }

    let preview: TrackPreview?
    var style: Style = .thumbnail
    var tag: String? = nil
    var tagColor: Color = OBCTheme.inkSoft
    var showsChrome: Bool = true
    var markers: [Marker] = []

    public init(
        _ preview: TrackPreview?,
        style: Style = .thumbnail,
        tag: String? = nil,
        tagColor: Color = OBCTheme.inkSoft,
        showsChrome: Bool = true,
        markers: [Marker] = []
    ) {
        self.preview = preview
        self.style = style
        self.tag = tag
        self.tagColor = tagColor
        self.showsChrome = showsChrome
        self.markers = markers
    }

    public var body: some View {
        canvas
            .background(OBCTheme.panel)
            .overlay(alignment: .topLeading) {
                if let tag {
                    Text(tag.uppercased())
                        .font(.obcMono(size: 10, weight: .bold))
                        .kerning(1)
                        .foregroundStyle(tagColor)
                        .padding(.vertical, 5)
                        .padding(.horizontal, 7)
                        .background(OBCTheme.panel.opacity(0.9))
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                        .overlay(
                            RoundedRectangle(cornerRadius: 6).strokeBorder(OBCTheme.line)
                        )
                        .padding(10)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: showsChrome ? OBCTheme.radiusPanel : 0))
            .overlay {
                if showsChrome {
                    RoundedRectangle(cornerRadius: OBCTheme.radiusPanel)
                        .strokeBorder(OBCTheme.line)
                }
            }
    }

    private var canvas: some View {
        Canvas { context, size in
            drawGrid(in: &context, size: size)

            guard let preview, !preview.points.isEmpty else {
                drawPlaceholderGlyph(in: &context, size: size)
                return
            }

            let transform = Self.fittingTransform(
                for: preview,
                in: size,
                inset: style.dotRadius + 6,
                topInset: tag == nil ? 0 : Self.tagBandHeight
            )
            let points = preview.points.map { transform($0) }

            if points.count > 1 {
                var path = Path()
                path.addLines(points)
                context.stroke(
                    path,
                    with: .color(OBCTheme.trackHalo),
                    style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round)
                )
                context.stroke(
                    path,
                    with: .color(OBCTheme.trackStroke),
                    style: StrokeStyle(lineWidth: 3.4, lineCap: .round, lineJoin: .round)
                )
            }

            if let first = points.first {
                drawNode(in: &context, at: first, fill: OBCTheme.trackStart)
            }
            if points.count > 1, let last = points.last {
                drawNode(in: &context, at: last, fill: OBCTheme.trackEnd)
            }

            for marker in markers {
                drawMarker(in: &context, at: transform(marker.point), label: marker.label)
            }
        }
    }

    /// The corner tag's height with its padding. A tagged fit starts below it, so the tag never
    /// covers a node dot.
    static let tagBandHeight: CGFloat = 34

    /// Maps unit-square track points into `size`, keeping the source aspect ratio
    /// (centred letterbox), with a uniform `inset` so round caps and node dots never
    /// clip, below a `topInset` band. Internal for the geometry unit tests.
    static func fittingTransform(
        for preview: TrackPreview,
        in size: CGSize,
        inset: CGFloat,
        topInset: CGFloat = 0
    ) -> (TrackPreview.Point) -> CGPoint {
        let available = CGSize(
            width: max(size.width - 2 * inset, 1),
            height: max(size.height - 2 * inset - topInset, 1)
        )
        let aspect = preview.aspectRatio > 0 ? preview.aspectRatio : 1
        // Fit a rect of the track's aspect into the available box.
        var fitted = CGSize(width: available.width, height: available.width / aspect)
        if fitted.height > available.height {
            fitted = CGSize(width: available.height * aspect, height: available.height)
        }
        let origin = CGPoint(
            x: (size.width - fitted.width) / 2,
            y: topInset + (size.height - topInset - fitted.height) / 2
        )
        return { point in
            CGPoint(
                x: origin.x + point.x * fitted.width,
                y: origin.y + point.y * fitted.height
            )
        }
    }

    private func drawGrid(in context: inout GraphicsContext, size: CGSize) {
        let step: CGFloat = 22
        var path = Path()
        var x = (size.width / 2).truncatingRemainder(dividingBy: step)
        while x < size.width {
            path.move(to: CGPoint(x: x, y: 0))
            path.addLine(to: CGPoint(x: x, y: size.height))
            x += step
        }
        var y = (size.height / 2).truncatingRemainder(dividingBy: step)
        while y < size.height {
            path.move(to: CGPoint(x: 0, y: y))
            path.addLine(to: CGPoint(x: size.width, y: y))
            y += step
        }
        context.stroke(path, with: .color(OBCTheme.gridLine), lineWidth: 1)
    }

    private func drawNode(in context: inout GraphicsContext, at point: CGPoint, fill: Color) {
        let r = style.dotRadius
        let ring = CGRect(x: point.x - r - 1.25, y: point.y - r - 1.25, width: 2 * (r + 1.25), height: 2 * (r + 1.25))
        context.fill(Path(ellipseIn: ring), with: .color(OBCTheme.panel))
        let dot = CGRect(x: point.x - r, y: point.y - r, width: 2 * r, height: 2 * r)
        context.fill(Path(ellipseIn: dot), with: .color(fill))
    }

    private func drawMarker(in context: inout GraphicsContext, at point: CGPoint, label: String) {
        let r: CGFloat = 9
        let ring = CGRect(x: point.x - r - 1.25, y: point.y - r - 1.25, width: 2 * (r + 1.25), height: 2 * (r + 1.25))
        context.fill(Path(ellipseIn: ring), with: .color(OBCTheme.panel))
        let dot = CGRect(x: point.x - r, y: point.y - r, width: 2 * r, height: 2 * r)
        context.fill(Path(ellipseIn: dot), with: .color(OBCTheme.amber))
        context.draw(
            Text(label).font(.obcMono(size: 10, weight: .bold)).foregroundColor(OBCTheme.ink),
            at: point
        )
    }

    /// The zigzag route glyph for no geometry, the same mark the empty state uses.
    private func drawPlaceholderGlyph(in context: inout GraphicsContext, size: CGSize) {
        let side = min(size.width, size.height) * 0.4
        let origin = CGPoint(x: (size.width - side) / 2, y: (size.height - side) / 2)
        // Design glyph "M4 19 8 6l4 9 4-11 4 15" in a 24pt box, scaled.
        let s = side / 24
        var path = Path()
        path.move(to: CGPoint(x: origin.x + 4 * s, y: origin.y + 19 * s))
        path.addLine(to: CGPoint(x: origin.x + 8 * s, y: origin.y + 6 * s))
        path.addLine(to: CGPoint(x: origin.x + 12 * s, y: origin.y + 15 * s))
        path.addLine(to: CGPoint(x: origin.x + 16 * s, y: origin.y + 4 * s))
        path.addLine(to: CGPoint(x: origin.x + 20 * s, y: origin.y + 19 * s))
        context.stroke(
            path,
            with: .color(OBCTheme.trackStroke),
            style: StrokeStyle(lineWidth: 1.8 * s, lineCap: .round, lineJoin: .round)
        )
    }
}

#Preview("Track preview") {
    VStack(spacing: 16) {
        TrackPreviewView(.obcSample, style: .hero, tag: "Planned")
            .frame(height: 214)
        HStack(spacing: 16) {
            TrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
            TrackPreviewView(nil)
                .frame(width: 128, height: 116)
        }
    }
    .padding()
    .background(OBCTheme.parchment)
}

extension TrackPreview {
    /// Preview and gallery sample: a real Kettle Moraine loop, so the grid fallback
    /// and the basemap both render a plausible track. Built through `normalizing` so
    /// `points` and `coordinates` stay aligned.
    public static let obcSample = TrackPreview.normalizing([
        .init(latitude: 42.905, longitude: -88.520), .init(latitude: 42.918, longitude: -88.505),
        .init(latitude: 42.930, longitude: -88.498), .init(latitude: 42.938, longitude: -88.478),
        .init(latitude: 42.929, longitude: -88.455), .init(latitude: 42.915, longitude: -88.447),
        .init(latitude: 42.902, longitude: -88.458), .init(latitude: 42.896, longitude: -88.480),
        .init(latitude: 42.905, longitude: -88.500), .init(latitude: 42.918, longitude: -88.512),
        .init(latitude: 42.905, longitude: -88.520),
    ])
}
