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

        public init(coordinates: [Coordinate], color: Color) {
            self.coordinates = coordinates
            self.color = color
        }
    }

    let stages: [Stage]
    var showsChrome: Bool

    @Environment(\.obcIsOnline) private var isOnline

    public init(stages: [Stage], showsChrome: Bool = true) {
        self.stages = stages
        self.showsChrome = showsChrome
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
    /// so they stay in register, then stroked in its color over gridded parchment.
    private var grid: some View {
        let shared = TrackPreview.normalizingShared(stages.map(\.coordinates))
        return Canvas { context, size in
            drawGrid(in: &context, size: size)
            // They share one aspect ratio, so take the first non-empty stage's.
            let aspect = shared.first { !$0.points.isEmpty }?.aspectRatio ?? 1
            let reference = TrackPreview(points: [], aspectRatio: aspect)
            let transform = TrackPreviewView.fittingTransform(for: reference, in: size, inset: 8)
            for (index, preview) in shared.enumerated() where preview.points.count > 1 {
                let points = preview.points.map { transform($0) }
                var path = Path()
                path.addLines(points)
                context.stroke(
                    path, with: .color(OBCTheme.trackHalo),
                    style: StrokeStyle(lineWidth: 6, lineCap: .round, lineJoin: .round))
                context.stroke(
                    path, with: .color(stages[index].color),
                    style: StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round))
            }
        }
        .background(OBCTheme.panel)
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
        context.stroke(path, with: .color(OBCTheme.gridLine), lineWidth: 1)
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
                MapPolyline(coordinates: coords)
                    .stroke(OBCTheme.trackHalo, style: StrokeStyle(lineWidth: 6, lineCap: .round, lineJoin: .round))
                MapPolyline(coordinates: coords)
                    .stroke(stage.color, style: StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round))
            }
        }
        // `initialPosition` is read once per Map identity, so key the identity on the
        // stage geometry. Without this, a route added to the trip lay outside the
        // frozen camera until the next app launch.
        .id(stages.map(\.coordinates))
        // Light tiles always; the palette is light throughout.
        .preferredColorScheme(.light)
        .allowsHitTesting(false)
        .modifier(PreviewChrome(showsChrome: showsChrome))
        #else
        grid
        #endif
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
                    RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
                }
            }
    }
}
