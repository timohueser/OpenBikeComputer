import SwiftUI
import OBCDomain

/// One waypoints-list row: a numbered marker, name and note, and the distance along.
/// The marker is ink for the first point, rust for the last, olive between.
public struct WaypointRow: View {
    let waypoint: Waypoint
    let isFirst: Bool
    let isLast: Bool
    var showsDivider = true

    public init(waypoint: Waypoint, isFirst: Bool = false, isLast: Bool = false, showsDivider: Bool = true) {
        self.waypoint = waypoint
        self.isFirst = isFirst
        self.isLast = isLast
        self.showsDivider = showsDivider
    }

    private var markerColors: (fill: Color, text: Color) {
        if isFirst { return (OBCTheme.ink, OBCTheme.surface) }
        if isLast { return (OBCTheme.rust, OBCTheme.onRust) }
        return (OBCTheme.secondary, OBCTheme.surface)
    }

    public var body: some View {
        HStack(spacing: 13) {
            Text("\(waypoint.index + 1)")
                .font(.system(.caption, weight: .semibold).monospacedDigit())
                .foregroundStyle(markerColors.text)
                .frame(width: 30, height: 30)
                .background(markerColors.fill)
                .clipShape(RoundedRectangle(cornerRadius: 9))
                .obcFixedGeometryType()

            VStack(alignment: .leading, spacing: 2) {
                Text(waypoint.name)
                    .font(.system(.subheadline, weight: .medium))
                    .foregroundStyle(OBCTheme.ink)
                    .lineLimit(1)
                if let note = waypoint.note {
                    Text(note)
                        .font(.system(.caption).monospacedDigit())
                        .foregroundStyle(OBCTheme.secondary)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Text(OBCFormat.distance(meters: waypoint.distanceAlongMeters))
                .font(.system(.caption).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
        }
        .padding(.vertical, 13)
        .padding(.horizontal, 2)
        .overlay(alignment: .bottom) {
            if showsDivider { OBCTheme.hairline.frame(height: 1) }
        }
    }
}

/// The waypoints dropdown under the disclosure row on route detail: the rows in ride
/// order plus the provenance footer. The track pins live on the detail hero itself.
public struct WaypointsDropdownContent: View {
    let waypoints: [Waypoint]

    public init(waypoints: [Waypoint]) {
        self.waypoints = waypoints
    }

    public var body: some View {
        VStack(spacing: 0) {
            ForEach(waypoints) { waypoint in
                WaypointRow(
                    waypoint: waypoint,
                    isFirst: waypoint.index == waypoints.first?.index,
                    isLast: waypoint.index == waypoints.last?.index,
                    showsDivider: waypoint.index != waypoints.last?.index
                )
            }
            Text("Waypoints come from the route file and are uploaded to the device with it.")
                .font(.system(.caption))
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: .infinity)
                .padding(.top, 10)
                .padding(.bottom, 4)
        }
    }
}

extension TrackPreviewView.Marker {
    /// Middle waypoints pinned on the polyline; the start and end already have node
    /// dots. The position is the track point nearest the waypoint's fraction of total
    /// distance: a preview-grade approximation, honest while the polyline sampling is
    /// roughly uniform, which it is because `TrackPreview` downsamples by a uniform
    /// stride.
    static func middleWaypointPins(
        _ waypoints: [Waypoint],
        on preview: TrackPreview?,
        totalDistanceMeters: Double
    ) -> [TrackPreviewView.Marker] {
        guard let preview, preview.points.count > 1, totalDistanceMeters > 0 else { return [] }
        return waypoints.dropFirst().dropLast().map { waypoint in
            let fraction = waypoint.distanceAlongMeters / totalDistanceMeters
            let index = pointIndex(fraction: fraction, pointCount: preview.points.count)
            return TrackPreviewView.Marker(
                id: waypoint.index,
                point: preview.points[index],
                label: "\(waypoint.index + 1)"
            )
        }
    }

    /// The marker-placement rule, exposed for unit tests.
    static func pointIndex(fraction: Double, pointCount: Int) -> Int {
        Int((max(0, min(fraction, 1)) * Double(pointCount - 1)).rounded())
    }
}

#Preview("Waypoints dropdown") {
    ScrollView {
        WaypointsDropdownContent(
            waypoints: [
                Waypoint(index: 0, name: "Ottawa Lake trailhead", note: "Start · parking & water", distanceAlongMeters: 0, coordinate: .init(latitude: 42.9, longitude: -88.6)),
                Waypoint(index: 1, name: "Emma Carlin junction", note: "Water · trail crossing", distanceAlongMeters: 14200, coordinate: .init(latitude: 42.9, longitude: -88.5)),
                Waypoint(index: 2, name: "Bald Bluff overlook", note: "Summit · 12% pitch before", distanceAlongMeters: 31600, coordinate: .init(latitude: 42.9, longitude: -88.4)),
                Waypoint(index: 3, name: "Ottawa Lake", note: "Finish", distanceAlongMeters: 62400, coordinate: .init(latitude: 42.9, longitude: -88.6)),
            ]
        )
        .padding(20)
    }
    .background(OBCTheme.page)
}
