import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// **Track preview with a real basemap** (#294) — a drop-in for `TrackPreviewView`
/// that draws the route/ride polyline over Apple Maps when there's a network path
/// and real geometry, and falls back to the grid + parchment placeholder
/// otherwise (offline, or a track that only kept its normalized shape). The
/// fallback is intentional, not a failure state — see `companion-ios/CLAUDE.md`.
///
/// Non-interactive at every size: the map ignores hits so a tap reaches the
/// enclosing card/hero button (the detail hero opens the full interactive
/// `TrackMapView`; a card still opens the detail screen).
///
/// Same call surface as `TrackPreviewView` (style / tag / chrome / markers) so
/// the swap is mechanical. Waypoint `markers` are unit-space (no lat/lon), so a
/// preview that carries them stays on the grid — the numbered-pin W1 schematic
/// isn't a basemap job.
public struct MapTrackPreviewView: View {
    let preview: TrackPreview?
    var style: TrackPreviewView.Style = .thumbnail
    var tag: String? = nil
    var tagColor: Color = OBCTheme.inkSoft
    var showsChrome: Bool = true
    var markers: [TrackPreviewView.Marker] = []

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        _ preview: TrackPreview?,
        style: TrackPreviewView.Style = .thumbnail,
        tag: String? = nil,
        tagColor: Color = OBCTheme.inkSoft,
        showsChrome: Bool = true,
        markers: [TrackPreviewView.Marker] = []
    ) {
        self.preview = preview
        self.style = style
        self.tag = tag
        self.tagColor = tagColor
        self.showsChrome = showsChrome
        self.markers = markers
    }

    private var mode: MapPreviewMode {
        // Unit-space markers can't sit on a basemap → keep those on the grid.
        guard markers.isEmpty else { return .grid }
        let hasCoordinates = !(preview?.coordinates.isEmpty ?? true)
        return MapPreviewMode.resolve(isOnline: isOnline, hasCoordinates: hasCoordinates)
    }

    public var body: some View {
        switch mode {
        case .grid:
            TrackPreviewView(
                preview, style: style, tag: tag, tagColor: tagColor,
                showsChrome: showsChrome, markers: markers
            )
            .accessibilityIdentifier("trackPreview.grid")
        case .map:
            mapPreview
                .accessibilityIdentifier("trackPreview.map")
        }
    }

    @ViewBuilder
    private var mapPreview: some View {
        #if canImport(MapKit)
        let coordinates = preview?.coordinates ?? []
        Map(
            initialPosition: .region(MapGeometry.boundingRegion(for: coordinates)),
            interactionModes: []
        ) {
            TrackMapContent(coordinates: coordinates, dotRadius: style.dotRadius)
        }
        // Standard Apple Maps styling (the constraints rule out reskinning it),
        // but always the light tile set — the design's field-guide palette is
        // light throughout, and Maps' dark tiles clash with the parchment
        // chrome around it regardless of the system appearance.
        .preferredColorScheme(.light)
        // The preview never handles gestures — the tap belongs to the card/hero.
        .allowsHitTesting(false)
        .overlay(alignment: .topLeading) {
            if let tag { MapPreviewTag(tag, color: tagColor) }
        }
        .clipShape(RoundedRectangle(cornerRadius: showsChrome ? OBCTheme.radiusPanel : 0))
        .overlay {
            if showsChrome {
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
            }
        }
        #else
        TrackPreviewView(
            preview, style: style, tag: tag, tagColor: tagColor,
            showsChrome: showsChrome, markers: markers
        )
        #endif
    }
}

#if canImport(MapKit)
/// The track polyline (halo + stroke) plus forest/coral start/end dots — shared
/// by the preview and the full-screen `TrackMapView` so both look identical.
struct TrackMapContent: MapContent {
    let coordinates: [Coordinate]
    var dotRadius: CGFloat = 5

    var body: some MapContent {
        let coords = MapGeometry.clLocations(coordinates)
        // Halo casing under the stroke — mirrors the grid preview's 7 / 3.4 pt.
        MapPolyline(coordinates: coords)
            .stroke(OBCTheme.trackHalo, style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
        MapPolyline(coordinates: coords)
            .stroke(OBCTheme.trackStroke, style: StrokeStyle(lineWidth: 3.4, lineCap: .round, lineJoin: .round))
        if let first = coords.first {
            Annotation("", coordinate: first) { nodeDot(OBCTheme.trackStart) }
        }
        if coords.count > 1, let last = coords.last {
            Annotation("", coordinate: last) { nodeDot(OBCTheme.trackEnd) }
        }
    }

    private func nodeDot(_ fill: Color) -> some View {
        Circle()
            .fill(fill)
            .frame(width: dotRadius * 2, height: dotRadius * 2)
            .overlay(Circle().strokeBorder(OBCTheme.panel, lineWidth: 2.5))
    }
}
#endif

/// The corner tag badge, matching `TrackPreviewView`'s (mono uppercase on a
/// panel chip) so a card reads the same whether it drew a map or the grid.
struct MapPreviewTag: View {
    let text: String
    let color: Color

    init(_ text: String, color: Color) {
        self.text = text
        self.color = color
    }

    var body: some View {
        Text(text.uppercased())
            .font(.obcMono(size: 10, weight: .bold))
            .kerning(1)
            .foregroundStyle(color)
            .padding(.vertical, 5)
            .padding(.horizontal, 7)
            .background(OBCTheme.panel.opacity(0.9))
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(OBCTheme.line))
            .padding(10)
    }
}

#if DEBUG
#Preview("Map track preview") {
    VStack(spacing: 16) {
        MapTrackPreviewView(.obcSample, style: .hero, tag: "Planned")
            .frame(height: 214)
        HStack(spacing: 16) {
            MapTrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
            // Forced offline → grid fallback.
            MapTrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
                .environment(\.obcIsOnline, false)
        }
    }
    .padding()
    .background(OBCTheme.parchment)
}
#endif
