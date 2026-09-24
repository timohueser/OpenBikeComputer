import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// The trip card's multi-stage preview: every stage of a trip on one preview in its
/// palette color. A sibling of ``MapTrackPreviewView`` that draws N polylines instead
/// of one, under the same rule: a MapKit basemap when there is a network path and
/// real geometry, the grid fallback otherwise. Non-interactive at every size, so a
/// tap reaches the enclosing trip card.
public struct MultiTrackPreviewView: View {
    public struct Stage: Equatable, Sendable {
        public let coordinates: [Coordinate]
        public let color: Color
        /// A dashed stage draws thinner and without the halo.
        public let dash: [CGFloat]

        public init(coordinates: [Coordinate], color: Color, dash: [CGFloat] = []) {
            self.coordinates = coordinates
            self.color = color
            self.dash = dash
        }

        var lineWidth: CGFloat { dash.isEmpty ? 3.4 : 2 }
    }

    /// A round mark on the preview, with an optional symbol in it.
    public struct Pin: Equatable, Sendable {
        public let coordinate: Coordinate
        public let color: Color
        public let systemImage: String?

        public init(coordinate: Coordinate, color: Color, systemImage: String? = nil) {
            self.coordinate = coordinate
            self.color = color
            self.systemImage = systemImage
        }
    }

    let stages: [Stage]
    let pins: [Pin]
    var showsChrome: Bool
    /// The route law's ends: an ink dot where the first stage starts, a rust square where the
    /// last one ends.
    var showsEnds: Bool

    @Environment(\.obcIsOnline) private var isOnline

    public init(stages: [Stage], pins: [Pin] = [], showsChrome: Bool = true, showsEnds: Bool = false) {
        self.stages = stages
        self.pins = pins
        self.showsChrome = showsChrome
        self.showsEnds = showsEnds
    }

    private var ends: (start: Coordinate, end: Coordinate)? {
        guard showsEnds, let start = stages.first(where: { !$0.coordinates.isEmpty })?.coordinates.first,
            let end = stages.last(where: { !$0.coordinates.isEmpty })?.coordinates.last
        else { return nil }
        return (start, end)
    }

    private var mode: MapPreviewMode {
        let hasCoordinates = stages.contains { !$0.coordinates.isEmpty }
        return MapPreviewMode.resolve(isOnline: isOnline, hasCoordinates: hasCoordinates)
    }

    public var body: some View {
        switch mode {
        case .grid:
            grid.accessibilityIdentifier("tripPreview.grid")
        case .map:
            map.accessibilityIdentifier("tripPreview.map")
        }
    }

    // MARK: Grid fallback

    /// The basemap-free fallback: every stage normalized into one shared unit square,
    /// so they stay in register, then stroked in its color over the gridded sketch ground.
    private var grid: some View {
        let shared = TrackPreview.normalizingShared(stages.map(\.coordinates) + pins.map { [$0.coordinate] })
        return Canvas { context, size in
            drawGrid(in: &context, size: size)
            // They share one aspect ratio, so take the first non-empty stage's.
            let aspect = shared.first { !$0.points.isEmpty }?.aspectRatio ?? 1
            let reference = TrackPreview(points: [], aspectRatio: aspect)
            let transform = TrackPreviewView.fittingTransform(for: reference, in: size, inset: 8)
            for (index, stage) in stages.enumerated() where shared[index].points.count > 1 {
                var path = Path()
                path.addLines(shared[index].points.map { transform($0) })
                if stage.dash.isEmpty {
                    context.stroke(
                        path, with: .color(OBCTheme.routeCasing),
                        style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
                }
                context.stroke(
                    path, with: .color(stage.color),
                    style: StrokeStyle(lineWidth: stage.lineWidth, lineCap: .round, lineJoin: .round, dash: stage.dash))
            }
            if ends != nil, let start = shared.first(where: { !$0.points.isEmpty })?.points.first.map(transform),
                let end = shared.prefix(stages.count).last(where: { !$0.points.isEmpty })?.points.last.map(transform) {
                let dot = Path(ellipseIn: CGRect(x: start.x - 4.5, y: start.y - 4.5, width: 9, height: 9))
                context.fill(dot, with: .color(OBCTheme.ink))
                context.stroke(dot, with: .color(OBCTheme.surface), lineWidth: 1.5)
                let square = Path(roundedRect: CGRect(x: end.x - 4.5, y: end.y - 4.5, width: 9, height: 9), cornerRadius: 2)
                context.fill(square, with: .color(OBCTheme.rust))
                context.stroke(square, with: .color(OBCTheme.surface), lineWidth: 1.5)
            }
            for (index, pin) in pins.enumerated() {
                guard let point = shared[stages.count + index].points.first.map(transform) else { continue }
                if let systemImage = pin.systemImage {
                    let disc = Path(ellipseIn: CGRect(x: point.x - 10, y: point.y - 10, width: 20, height: 20))
                    context.fill(disc, with: .color(OBCTheme.surface))
                    context.stroke(disc, with: .color(OBCTheme.hairlineStrong))
                    var image = context.resolve(Image(systemName: systemImage))
                    image.shading = .color(pin.color)
                    context.draw(image, in: CGRect(x: point.x - 6, y: point.y - 6, width: 12, height: 12))
                } else {
                    let dot = Path(ellipseIn: CGRect(x: point.x - 4, y: point.y - 4, width: 8, height: 8))
                    context.fill(dot, with: .color(pin.color))
                    context.stroke(dot, with: .color(.white), lineWidth: 1.2)
                }
            }
        }
        .background(OBCTheme.sketchGround)
        .modifier(PreviewChrome(showsChrome: showsChrome))
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
        context.stroke(path, with: .color(OBCTheme.sketchLine), lineWidth: 1)
    }

    // MARK: MapKit basemap

    @ViewBuilder
    private var map: some View {
        #if canImport(MapKit)
        let allCoordinates = stages.flatMap(\.coordinates)
        Map(
            initialPosition: .region(MapGeometry.boundingRegion(for: allCoordinates)),
            interactionModes: []
        ) {
            ForEach(Array(stages.enumerated()), id: \.offset) { _, stage in
                let coords = MapGeometry.clLocations(stage.coordinates)
                if stage.dash.isEmpty {
                    MapPolyline(coordinates: coords)
                        .stroke(OBCTheme.routeCasing, style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
                }
                MapPolyline(coordinates: coords)
                    .stroke(stage.color, style: StrokeStyle(
                        lineWidth: stage.lineWidth, lineCap: .round, lineJoin: .round, dash: stage.dash))
            }
            if let ends {
                Annotation("", coordinate: MapGeometry.clLocations([ends.start])[0]) {
                    EndMark(shape: Circle(), fill: OBCTheme.ink)
                }
                Annotation("", coordinate: MapGeometry.clLocations([ends.end])[0]) {
                    EndMark(shape: RoundedRectangle(cornerRadius: 2), fill: OBCTheme.rust)
                }
            }
            ForEach(Array(pins.enumerated()), id: \.offset) { _, pin in
                Annotation("", coordinate: MapGeometry.clLocations([pin.coordinate])[0], anchor: .center) {
                    PinMark(pin: pin)
                }
            }
        }
        // `initialPosition` is read once per Map identity, so key the identity on the
        // stage geometry. Without this, a route added to the trip lay outside the
        // frozen camera until the next app launch.
        .id(stages.map(\.coordinates))
        .allowsHitTesting(false)
        .modifier(PreviewChrome(showsChrome: showsChrome))
        #else
        grid
        #endif
    }
}

/// A route end on the map: the ink start dot or the rust end square.
private struct EndMark<S: InsettableShape>: View {
    let shape: S
    let fill: Color

    var body: some View {
        shape.fill(fill)
            .frame(width: 10, height: 10)
            .overlay(shape.strokeBorder(OBCTheme.surface, lineWidth: 2))
    }
}

/// A pin: a filled circle with a white rim, or a surface disc with the symbol.
private struct PinMark: View {
    let pin: MultiTrackPreviewView.Pin

    var body: some View {
        if let systemImage = pin.systemImage {
            Image(systemName: systemImage)
                .font(.system(.caption2, weight: .semibold))
                .foregroundStyle(pin.color)
                .frame(width: 20, height: 20)
                .background(Circle().fill(OBCTheme.surface))
                .overlay(Circle().strokeBorder(OBCTheme.hairlineStrong))
        } else {
            Circle().fill(pin.color)
                .frame(width: 8, height: 8)
                .overlay(Circle().strokeBorder(.white, lineWidth: 1.2))
        }
    }
}

/// The card chrome shared by both preview modes, matching ``TrackPreviewView``'s.
private struct PreviewChrome: ViewModifier {
    let showsChrome: Bool

    func body(content: Content) -> some View {
        content
            .clipShape(RoundedRectangle(cornerRadius: showsChrome ? OBCTheme.radiusPanel : 0))
            .overlay {
                if showsChrome {
                    RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline)
                }
            }
    }
}
