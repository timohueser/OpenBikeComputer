import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// A drop-in for `TrackPreviewView` that draws the track over the OSM basemap when there
/// is a network path and real geometry, and falls back to the track sketch
/// otherwise. The fallback is intentional, not a failure state.
///
/// Non-interactive at every size: the map ignores hits, so a tap reaches the
/// enclosing card or hero button.
///
/// Pass `waypoints` and the total distance to pin the middle waypoints. On the
/// basemap they sit at their real coordinates; on the grid they fall back to
/// distance-fraction placement.
public struct MapTrackPreviewView: View {
    let preview: TrackPreview?
    var ink: TrackPreviewView.Ink = .route
    var style: TrackPreviewView.Style = .thumbnail
    var showsChrome: Bool = true
    var waypoints: [Waypoint] = []
    /// Only needed to place `waypoints` on the grid.
    var totalDistanceMeters: Double = 0
    /// A ride's photos. The basemap pins them; the grid does not.
    var photoPins: [Coordinate] = []
    /// The index in `photoPins` of the photo on screen.
    var highlightedPhoto: Int? = nil
    /// A ride timeline's cursor. The basemap marks it; the grid does not.
    var cursor: Coordinate? = nil

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        _ preview: TrackPreview?,
        ink: TrackPreviewView.Ink = .route,
        style: TrackPreviewView.Style = .thumbnail,
        showsChrome: Bool = true,
        waypoints: [Waypoint] = [],
        totalDistanceMeters: Double = 0,
        photoPins: [Coordinate] = [],
        highlightedPhoto: Int? = nil,
        cursor: Coordinate? = nil
    ) {
        self.preview = preview
        self.ink = ink
        self.style = style
        self.showsChrome = showsChrome
        self.waypoints = waypoints
        self.totalDistanceMeters = totalDistanceMeters
        self.photoPins = photoPins
        self.highlightedPhoto = highlightedPhoto
        self.cursor = cursor
    }

    private var mode: MapPreviewMode {
        let hasCoordinates = !(preview?.coordinates.isEmpty ?? true)
        return MapPreviewMode.resolve(isOnline: isOnline, hasCoordinates: hasCoordinates)
    }

    public var body: some View {
        switch mode {
        case .grid:
            TrackPreviewView(
                preview, ink: ink, style: style,
                showsChrome: showsChrome,
                markers: TrackPreviewView.Marker.middleWaypointPins(
                    waypoints, on: preview, totalDistanceMeters: totalDistanceMeters
                )
            )
            .accessibilityIdentifier("trackPreview.grid")
        case .map:
            mapPreview
                .accessibilityIdentifier("trackPreview.map")
        }
    }

    @ViewBuilder
    private var mapPreview: some View {
        #if os(iOS)
        let coordinates = preview?.coordinates ?? []
        OBCMapView(lines: [MapStroke(coordinates: coordinates, color: ink.color, cased: ink.cased)],
                   pins: mapPins(coordinates), interactive: false)
        .allowsHitTesting(false)
        .clipShape(RoundedRectangle(cornerRadius: showsChrome ? OBCTheme.radiusPanel : 0))
        .overlay {
            if showsChrome {
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline)
            }
        }
        #else
        TrackPreviewView(
            preview, ink: ink, style: style,
            showsChrome: showsChrome,
            markers: TrackPreviewView.Marker.middleWaypointPins(
                waypoints, on: preview, totalDistanceMeters: totalDistanceMeters
            )
        )
        #endif
    }
    #if os(iOS)
    private func mapPins(_ coordinates: [Coordinate]) -> [MapPin] {
        var pins: [MapPin] = []
        if let first = coordinates.first { pins.append(MapPin(coordinate: first, size: style.dotRadius * 2)) }
        if let last = coordinates.last, coordinates.count > 1 { pins.append(MapPin(coordinate: last, color: OBCTheme.rust, square: true, size: style.dotRadius * 2)) }
        pins += waypoints.dropFirst().dropLast().map { MapPin(coordinate: $0.coordinate, color: OBCTheme.secondary, label: "\($0.index + 1)") }
        let photoOrder = photoPins.indices.filter { $0 != highlightedPhoto } + (highlightedPhoto.map { photoPins.indices.contains($0) ? [$0] : [] } ?? [])
        pins += photoOrder.map { MapPin(coordinate: photoPins[$0], color: OBCTheme.ride, photo: $0 == highlightedPhoto) }
        if let cursor { pins.append(MapPin(coordinate: cursor, color: OBCTheme.amber, size: 14)) }
        return pins
    }
    #endif

}

#if canImport(MapKit)
/// The numbered waypoint pin as a live view, olive as in the waypoints list. The sketch draws
/// the same mark in its `Canvas`.
struct WaypointPinBadge: View {
    let label: String

    var body: some View {
        Text(label)
            .font(.system(.caption2, weight: .semibold).monospacedDigit())
            .foregroundStyle(OBCTheme.surface)
            .frame(width: 18, height: 18)
            .background(Circle().fill(OBCTheme.secondary))
            .overlay(Circle().strokeBorder(OBCTheme.surface, lineWidth: 2.5))
            .obcFixedGeometryType()
    }
}

/// A photo's place on the map: a small dot, or a camera badge for the photo on screen.
struct PhotoPin: View {
    let highlighted: Bool

    var body: some View {
        Circle()
            .fill(OBCTheme.ride)
            .frame(width: highlighted ? 22 : 9, height: highlighted ? 22 : 9)
            .overlay {
                if highlighted {
                    Image(systemName: "camera.fill")
                        .font(.system(.caption2, weight: .semibold))
                        .foregroundStyle(OBCTheme.surface)
                }
            }
            .overlay(Circle().strokeBorder(OBCTheme.surface, lineWidth: highlighted ? 2.5 : 2))
    }
}
#endif

#if DEBUG
#Preview("Map track preview") {
    VStack(spacing: 16) {
        MapTrackPreviewView(.obcSample, style: .hero)
            .frame(height: 214)
        HStack(spacing: 16) {
            MapTrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
            MapTrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
                .environment(\.obcIsOnline, false)
        }
    }
    .padding()
    .background(OBCTheme.page)
}
#endif
