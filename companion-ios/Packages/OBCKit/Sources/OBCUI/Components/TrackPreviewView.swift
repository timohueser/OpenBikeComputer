import SwiftUI
import OBCDomain

/// **GPS Track Preview** (§9, NEW) — the basemap-free polyline on gridded
/// parchment that identifies every route/ride in the app. Renders the
/// normalized `TrackPreview` from `OBCDomain` (unit-square points, y-down),
/// letterboxed to the source aspect ratio. **Never a basemap** — epic
/// non-negotiable.
///
/// Design metrics (`.track` in the design HTML): panel face with a 22pt faint
/// grid, 7pt `trackHalo` casing under a 3.4pt `trackStroke` line, start dot in
/// forest / end dot in coral with a 2.5pt panel ring. Thumbnails use 4.5pt
/// dots, heroes 6pt — pass `style:`.
///
/// The card chrome (border + 14pt radius) is on by default; the compact route
/// card turns it off for its flush left cell.
public struct TrackPreviewView: View {
    public enum Style {
        /// 128pt-wide list-row cell — 4.5pt node dots.
        case thumbnail
        /// Detail-page hero — 6pt node dots.
        case hero

        var dotRadius: CGFloat {
            switch self {
            case .thumbnail: 4.5
            case .hero: 6
            }
        }
    }

    /// Extra dots pinned on the polyline (the waypoints screen W1) — a unit
    /// point plus a label drawn in an amber marker.
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
                inset: style.dotRadius + 6
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

    /// Maps unit-square track points into `size`, preserving the source aspect
    /// ratio (centered letterbox) with a uniform `inset` so round caps and node
    /// dots never clip. Internal for the geometry unit tests.
    static func fittingTransform(
        for preview: TrackPreview,
        in size: CGSize,
        inset: CGFloat
    ) -> (TrackPreview.Point) -> CGPoint {
        let available = CGSize(
            width: max(size.width - 2 * inset, 1),
            height: max(size.height - 2 * inset, 1)
        )
        let aspect = preview.aspectRatio > 0 ? preview.aspectRatio : 1
        // Fit a rect of the track's aspect into the available box.
        var fitted = CGSize(width: available.width, height: available.width / aspect)
        if fitted.height > available.height {
            fitted = CGSize(width: available.height * aspect, height: available.height)
        }
        let origin = CGPoint(
            x: (size.width - fitted.width) / 2,
            y: (size.height - fitted.height) / 2
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
        // Centered like the design's `background-position:center`.
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

    /// W1's numbered waypoint pin: 9pt amber dot, panel ring, mono label.
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

    /// The zigzag route glyph shown when there is no geometry (loading or a
    /// genuinely empty track) — the same mark the empty state uses.
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
    /// Preview/gallery sample shaped like the design's Kettle Moraine curve.
    public static let obcSample = TrackPreview(
        points: [
            .init(x: 0.02, y: 0.85), .init(x: 0.08, y: 0.55), .init(x: 0.18, y: 0.42),
            .init(x: 0.30, y: 0.55), .init(x: 0.42, y: 0.78), .init(x: 0.55, y: 0.82),
            .init(x: 0.66, y: 0.62), .init(x: 0.76, y: 0.35), .init(x: 0.82, y: 0.18),
            .init(x: 0.92, y: 0.10), .init(x: 0.98, y: 0.05),
        ],
        aspectRatio: 1.35
    )
}
