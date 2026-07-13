import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// The **trip card's multi-stage preview** (TR6) — every stage of a trip drawn
/// on **one** preview in its palette color (`OBCTheme.stageColor(index:)`). A
/// drop-in sibling of ``MapTrackPreviewView`` that draws *N* polylines instead
/// of one, and follows the exact same #294 rule: a real MapKit basemap when
/// there's a network path **and** real geometry, the grid + parchment fallback
/// otherwise (offline, or stages that only kept their normalized shape).
///
/// Non-interactive at every size — the map ignores hits so a tap reaches the
/// enclosing trip card.
public struct MultiTrackPreviewView: View {
    /// One stage of the trip: its geometry and the palette color it draws in.
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

    /// The basemap-free fallback: every stage normalized into **one shared**
    /// unit square (so they stay in register) and stroked in its color over the
    /// same gridded parchment ``TrackPreviewView`` draws.
    private var grid: some View {
        let shared = TrackPreview.normalizingShared(stages.map(\.coordinates))
        return Canvas { context, size in
            drawGrid(in: &context, size: size)
            // All shares one aspect ratio — take the first non-empty stage's.
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
        // Light tiles always — the field-guide palette is light throughout (see
        // MapTrackPreviewView).
        .preferredColorScheme(.light)
        .allowsHitTesting(false)
        .modifier(PreviewChrome(showsChrome: showsChrome))
        #else
        grid
        #endif
    }
}

/// The card chrome (clip + hairline) shared by both preview modes, matching
/// ``TrackPreviewView``'s.
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
