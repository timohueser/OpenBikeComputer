import SwiftUI
import OBCDomain

/// The track sketch: the basemap-free drawing that identifies every route, trip and ride in a
/// list, and the detail pages' fallback when the map cannot load. Each line is fitted into the
/// source aspect ratio on the sage ground, a planned line in `route` over its `routeCasing`, a
/// ride bare in `ride`. A line starts at an ink dot and ends at a small rust square.
public struct TrackPreviewView: View {
    public enum Style {
        case thumbnail
        case hero

        /// The half-size of the start dot and the end square.
        var dotRadius: CGFloat {
            switch self {
            case .thumbnail: 3.5
            case .hero: 5
            }
        }

        var lineWidth: CGFloat {
            switch self {
            case .thumbnail: 2.5
            case .hero: 3.4
            }
        }

        var casingWidth: CGFloat {
            switch self {
            case .thumbnail: 5.5
            case .hero: 8
            }
        }
    }

    /// How a line is drawn: its colour, and whether it sits on the amber casing.
    public struct Ink: Sendable {
        public let color: Color
        public let cased: Bool

        public init(color: Color, cased: Bool) {
            self.color = color
            self.cased = cased
        }

        /// A planned route.
        public static let route = Ink(color: OBCTheme.route, cased: true)
        /// A recorded ride.
        public static let ride = Ink(color: OBCTheme.ride, cased: false)

        /// The trip day at `index` in ride order.
        public static func day(_ index: Int) -> Ink {
            Ink(color: OBCTheme.stageColor(index: index), cased: true)
        }
    }

    /// An extra dot pinned on the polyline: a unit point plus a label drawn in an
    /// olive marker.
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

    private struct Line {
        let points: [TrackPreview.Point]
        let ink: Ink
    }

    private let lines: [Line]
    private let aspectRatio: Double
    private let showsEnds: Bool
    /// The zigzag mark when there is nothing to draw. A many-track sketch stays bare instead.
    private let showsPlaceholder: Bool
    var style: Style = .thumbnail
    var showsChrome: Bool = true
    var markers: [Marker] = []

    public init(
        _ preview: TrackPreview?,
        ink: Ink = .route,
        style: Style = .thumbnail,
        showsChrome: Bool = true,
        markers: [Marker] = []
    ) {
        self.lines = preview.map { [Line(points: $0.points, ink: ink)] } ?? []
        self.aspectRatio = preview?.aspectRatio ?? 1
        self.showsEnds = true
        self.showsPlaceholder = true
        self.style = style
        self.showsChrome = showsChrome
        self.markers = markers
    }

    /// Several tracks in one frame, such as a trip's days or a year of rides. `showsEnds: false`
    /// leaves out the start and end marks, which crowd a sketch of many lines.
    public init(
        tracks: [(coordinates: [Coordinate], ink: Ink)],
        showsEnds: Bool = true,
        showsChrome: Bool = true
    ) {
        let shared = TrackPreview.normalizingShared(tracks.map(\.coordinates))
        self.lines = zip(shared, tracks).map { Line(points: $0.points, ink: $1.ink) }
        self.aspectRatio = shared.first { !$0.points.isEmpty }?.aspectRatio ?? 1
        self.showsEnds = showsEnds
        self.showsPlaceholder = false
        self.showsChrome = showsChrome
    }

    public var body: some View {
        canvas
            .background(OBCTheme.sketchGround)
            .clipShape(RoundedRectangle(cornerRadius: showsChrome ? OBCTheme.radiusPanel : 0))
            .overlay {
                if showsChrome {
                    RoundedRectangle(cornerRadius: OBCTheme.radiusPanel)
                        .strokeBorder(OBCTheme.hairline)
                }
            }
    }

    private var canvas: some View {
        Canvas { context, size in
            let drawn = lines.filter { !$0.points.isEmpty }
            guard !drawn.isEmpty else {
                if showsPlaceholder { drawPlaceholderGlyph(in: &context, size: size) }
                return
            }

            let transform = Self.fittingTransform(
                for: TrackPreview(points: [], aspectRatio: aspectRatio),
                in: size,
                inset: style.dotRadius + 3
            )
            let paths = drawn.map { line -> (Path, Ink) in
                var path = Path()
                path.addLines(line.points.map(transform))
                return (path, line.ink)
            }
            // Every casing first, so a later line never hides under an earlier one's casing.
            for (path, ink) in paths where ink.cased {
                context.stroke(path, with: .color(OBCTheme.routeCasing), style: stroke(style.casingWidth))
            }
            for (path, ink) in paths {
                context.stroke(path, with: .color(ink.color), style: stroke(style.lineWidth))
            }

            if showsEnds {
                for line in drawn {
                    drawStart(in: &context, at: transform(line.points[0]))
                    if line.points.count > 1, let last = line.points.last {
                        drawEnd(in: &context, at: transform(last))
                    }
                }
            }

            for marker in markers {
                drawMarker(in: &context, at: transform(marker.point), label: marker.label)
            }
        }
    }

    private func stroke(_ width: CGFloat) -> StrokeStyle {
        StrokeStyle(lineWidth: width, lineCap: .round, lineJoin: .round)
    }

    /// Maps unit-square track points into `size`, keeping the source aspect ratio
    /// (centred letterbox), with a uniform `inset` so round caps and node dots never
    /// clip. Internal for the geometry unit tests.
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

    /// The start: an ink dot, ringed in the ground colour so it reads over the line.
    private func drawStart(in context: inout GraphicsContext, at point: CGPoint) {
        let r = style.dotRadius
        let dot = Path(ellipseIn: CGRect(x: point.x - r, y: point.y - r, width: 2 * r, height: 2 * r))
        context.stroke(dot, with: .color(OBCTheme.sketchGround), lineWidth: 3)
        context.fill(dot, with: .color(OBCTheme.ink))
    }

    /// The end: a small rust square, as the device marks the finish.
    private func drawEnd(in context: inout GraphicsContext, at point: CGPoint) {
        let r = style.dotRadius
        let square = Path(
            roundedRect: CGRect(x: point.x - r, y: point.y - r, width: 2 * r, height: 2 * r),
            cornerRadius: 1
        )
        context.stroke(square, with: .color(OBCTheme.sketchGround), lineWidth: 3)
        context.fill(square, with: .color(OBCTheme.rust))
    }

    private func drawMarker(in context: inout GraphicsContext, at point: CGPoint, label: String) {
        let r: CGFloat = 9
        let ring = CGRect(x: point.x - r - 1.25, y: point.y - r - 1.25, width: 2 * (r + 1.25), height: 2 * (r + 1.25))
        context.fill(Path(ellipseIn: ring), with: .color(OBCTheme.surface))
        let dot = CGRect(x: point.x - r, y: point.y - r, width: 2 * r, height: 2 * r)
        context.fill(Path(ellipseIn: dot), with: .color(OBCTheme.secondary))
        context.draw(
            Text(label).font(.system(.caption2, weight: .semibold).monospacedDigit()).foregroundColor(OBCTheme.surface),
            at: point
        )
    }

    /// The zigzag route mark for no geometry, drawn like a route. The empty state uses it too.
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
        context.stroke(path, with: .color(OBCTheme.routeCasing), style: stroke(style.casingWidth))
        context.stroke(path, with: .color(OBCTheme.route), style: stroke(style.lineWidth))
        drawStart(in: &context, at: CGPoint(x: origin.x + 4 * s, y: origin.y + 19 * s))
        drawEnd(in: &context, at: CGPoint(x: origin.x + 20 * s, y: origin.y + 19 * s))
    }
}

#Preview("Track preview") {
    VStack(spacing: 16) {
        TrackPreviewView(.obcSample, style: .hero)
            .frame(height: 214)
        HStack(spacing: 16) {
            TrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
            TrackPreviewView(nil)
                .frame(width: 128, height: 116)
        }
    }
    .padding()
    .background(OBCTheme.page)
}

extension TrackPreview {
    /// Preview and gallery sample: a real Kettle Moraine loop, so the sketch
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
