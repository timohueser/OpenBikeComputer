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
    var style: TrackPreviewView.Style = .thumbnail
    var tag: String? = nil
    var tagColor: Color = OBCTheme.secondary
    var showsChrome: Bool = true
    var waypoints: [Waypoint] = []
    /// Only needed to place `waypoints` on the grid.
    var totalDistanceMeters: Double = 0
    /// A ride's photos. The basemap pins them; the grid does not.
    var photoPins: [Coordinate] = []
    /// The index in `photoPins` of the photo on screen.
    var highlightedPhoto: Int? = nil

    @Environment(\.obcIsOnline) private var isOnline

    public init(
        _ preview: TrackPreview?,
        style: TrackPreviewView.Style = .thumbnail,
        tag: String? = nil,
        tagColor: Color = OBCTheme.secondary,
        showsChrome: Bool = true,
        waypoints: [Waypoint] = [],
        totalDistanceMeters: Double = 0,
        photoPins: [Coordinate] = [],
        highlightedPhoto: Int? = nil
    ) {
        self.preview = preview
        self.style = style
        self.tag = tag
        self.tagColor = tagColor
        self.showsChrome = showsChrome
        self.waypoints = waypoints
        self.totalDistanceMeters = totalDistanceMeters
        self.photoPins = photoPins
        self.highlightedPhoto = highlightedPhoto
    }

    private var mode: MapPreviewMode {
        let hasCoordinates = !(preview?.coordinates.isEmpty ?? true)
        return MapPreviewMode.resolve(isOnline: isOnline, hasCoordinates: hasCoordinates)
    }

    public var body: some View {
        switch mode {
        case .grid:
            TrackPreviewView(
                preview, style: style, tag: tag, tagColor: tagColor,
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
                coordinates: coordinates, dotRadius: style.dotRadius, waypoints: waypoints,
                photoPins: photoPins, highlightedPhoto: highlightedPhoto
            )
        }
        // The camera frames the track inside the safe area, so this keeps it clear of the tag.
        .safeAreaPadding(.top, tag == nil ? 0 : TrackPreviewView.tagBandHeight)
        .allowsHitTesting(false)
        .overlay(alignment: .topLeading) {
            if let tag { MapPreviewTag(tag, color: tagColor) }
        }
        .clipShape(RoundedRectangle(cornerRadius: showsChrome ? OBCTheme.radiusPanel : 0))
        .overlay {
            if showsChrome {
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline)
            }
        }
        #else
        TrackPreviewView(
            preview, style: style, tag: tag, tagColor: tagColor,
            showsChrome: showsChrome,
            markers: TrackPreviewView.Marker.middleWaypointPins(
                waypoints, on: preview, totalDistanceMeters: totalDistanceMeters
            )
        )
        #endif
    }
}

#if canImport(MapKit)
/// The track polyline and start/end dots, shared by the preview and the full-screen
/// `TrackMapView` so both look identical. `waypoints` pins the middle waypoints; the
/// start and end already have dots.
struct TrackMapContent: MapContent {
    let coordinates: [Coordinate]
    var dotRadius: CGFloat = 5
    var waypoints: [Waypoint] = []
    var photoPins: [Coordinate] = []
    var highlightedPhoto: Int?

    var body: some MapContent {
        let coords = MapGeometry.clLocations(coordinates)
        // Halo casing under the stroke.
        MapPolyline(coordinates: coords)
            .stroke(OBCTheme.routeCasing, style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
        MapPolyline(coordinates: coords)
            .stroke(OBCTheme.route, style: StrokeStyle(lineWidth: 3.4, lineCap: .round, lineJoin: .round))
        if let first = coords.first {
            Annotation("", coordinate: first) { nodeDot(OBCTheme.ink) }
        }
        if coords.count > 1, let last = coords.last {
            Annotation("", coordinate: last) { nodeDot(OBCTheme.rust) }
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
    }

    private var photoPinOrder: [Int] {
        let others = photoPins.indices.filter { $0 != highlightedPhoto }
        guard let highlightedPhoto, photoPins.indices.contains(highlightedPhoto) else { return others }
        return others + [highlightedPhoto]
    }

    private func nodeDot(_ fill: Color) -> some View {
        Circle()
            .fill(fill)
            .frame(width: dotRadius * 2, height: dotRadius * 2)
            .overlay(Circle().strokeBorder(OBCTheme.surface, lineWidth: 2.5))
    }
}

/// The numbered waypoint pin as a live view. The grid preview draws the same mark
/// in its `Canvas`.
struct WaypointPinBadge: View {
    let label: String

    var body: some View {
        Text(label)
            .font(.system(.caption2, weight: .semibold).monospacedDigit())
            .foregroundStyle(OBCTheme.ink)
            .frame(width: 18, height: 18)
            .background(Circle().fill(OBCTheme.amber))
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

/// The corner tag badge, matching `TrackPreviewView`'s so a card reads the same
/// whether it drew a map or the grid.
struct MapPreviewTag: View {
    let text: String
    let color: Color

    init(_ text: String, color: Color) {
        self.text = text
        self.color = color
    }

    var body: some View {
        Text(text.uppercased())
            .font(.system(.caption2, weight: .semibold).monospacedDigit())
            .kerning(1)
            .foregroundStyle(color)
            .padding(.vertical, 5)
            .padding(.horizontal, 7)
            .background(OBCTheme.surface.opacity(0.9))
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(OBCTheme.hairline))
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
            MapTrackPreviewView(.obcSample)
                .frame(width: 128, height: 116)
                .environment(\.obcIsOnline, false)
        }
    }
    .padding()
    .background(OBCTheme.page)
}
#endif
