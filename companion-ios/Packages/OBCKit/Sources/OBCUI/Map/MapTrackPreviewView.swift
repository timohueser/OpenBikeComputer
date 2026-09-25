import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// A drop-in for `TrackPreviewView` that draws the track over Apple Maps when there
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
        #if canImport(MapKit)
        let coordinates = preview?.coordinates ?? []
        Map(
            initialPosition: .region(MapGeometry.boundingRegion(for: coordinates)),
            interactionModes: []
        ) {
            TrackMapContent(
                coordinates: coordinates, ink: ink, dotRadius: style.dotRadius, waypoints: waypoints,
                photoPins: photoPins, highlightedPhoto: highlightedPhoto, cursor: cursor
            )
        }
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
}

#if canImport(MapKit)
/// The track polyline, its ink start dot and rust end square, shared by the preview and the full-screen
/// `TrackMapView` so both look identical. `waypoints` pins the middle waypoints; the
/// start and end already have dots.
struct TrackMapContent: MapContent {
    let coordinates: [Coordinate]
    var ink: TrackPreviewView.Ink = .route
    var dotRadius: CGFloat = 5
    var waypoints: [Waypoint] = []
    var photoPins: [Coordinate] = []
    var highlightedPhoto: Int?
    var cursor: Coordinate?

    var body: some MapContent {
        let coords = MapGeometry.clLocations(coordinates)
        if ink.cased {
            MapPolyline(coordinates: coords)
                .stroke(OBCTheme.routeCasing, style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
        }
        MapPolyline(coordinates: coords)
            .stroke(ink.color, style: StrokeStyle(lineWidth: 3.4, lineCap: .round, lineJoin: .round))
        if let first = coords.first {
            Annotation("", coordinate: first) { endMark(Circle(), fill: OBCTheme.ink) }
        }
        if coords.count > 1, let last = coords.last {
            Annotation("", coordinate: last) { endMark(RoundedRectangle(cornerRadius: 2), fill: OBCTheme.rust) }
        }
        ForEach(Array(waypoints.dropFirst().dropLast())) { waypoint in
            Annotation(
                "",
                coordinate: CLLocationCoordinate2D(
                    latitude: waypoint.coordinate.latitude,
                    longitude: waypoint.coordinate.longitude
                )
            ) {
                WaypointPinBadge(label: "\(waypoint.index + 1)")
            }
        }
        // The highlighted pin comes last, so it draws over its neighbours.
        ForEach(photoPinOrder, id: \.self) { index in
            Annotation("", coordinate: MapGeometry.clLocations([photoPins[index]])[0]) {
                PhotoPin(highlighted: index == highlightedPhoto)
            }
        }
        if let cursor {
            Annotation("", coordinate: MapGeometry.clLocations([cursor])[0]) {
                Circle()
                    .fill(OBCTheme.amber)
                    .frame(width: 14, height: 14)
                    .overlay(Circle().strokeBorder(OBCTheme.surface, lineWidth: 2.5))
                    .shadow(color: .black.opacity(0.3), radius: 1.5, y: 1)
            }
        }
    }

    private var photoPinOrder: [Int] {
        let others = photoPins.indices.filter { $0 != highlightedPhoto }
        guard let highlightedPhoto, photoPins.indices.contains(highlightedPhoto) else { return others }
        return others + [highlightedPhoto]
    }

    private func endMark(_ shape: some InsettableShape, fill: Color) -> some View {
        shape
            .fill(fill)
            .frame(width: dotRadius * 2, height: dotRadius * 2)
            .overlay(shape.strokeBorder(OBCTheme.surface, lineWidth: 2))
    }
}

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
