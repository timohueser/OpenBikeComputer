import SwiftUI
import OBCDomain

/// One waypoints-list row: a numbered 30pt marker (9pt radius), name + mono
/// note, and the mono distance-along. Marker color: forest for the first
/// point, coral for the last, amber between.
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

    private var markerColor: Color {
        if isFirst { return OBCTheme.trackStart }
        if isLast { return OBCTheme.trackEnd }
        return OBCTheme.amber
    }

    public var body: some View {
        HStack(spacing: 13) {
            Text("\(waypoint.index + 1)")
                .font(.obcMono(size: 12, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 30, height: 30)
                .background(markerColor)
                .clipShape(RoundedRectangle(cornerRadius: 9))

            VStack(alignment: .leading, spacing: 2) {
                Text(waypoint.name)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(OBCTheme.ink)
                    .lineLimit(1)
                if let note = waypoint.note {
                    Text(note)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Text(OBCFormat.distance(meters: waypoint.distanceAlongMeters))
                .font(.obcMono(size: 12))
                .foregroundStyle(OBCTheme.inkFaint)
        }
        .padding(.vertical, 13)
        .padding(.horizontal, 2)
        .overlay(alignment: .bottom) {
            if showsDivider { OBCTheme.screenLine.frame(height: 1) }
        }
    }
}

/// The full waypoints-screen content: the mini track with the middle
/// waypoints pinned as numbered amber markers, then the rows in ride order.
public struct WaypointsListView: View {
    let waypoints: [Waypoint]
    let preview: TrackPreview?
    /// Total route distance, used to place markers along the polyline.
    let totalDistanceMeters: Double

    public init(waypoints: [Waypoint], preview: TrackPreview?, totalDistanceMeters: Double) {
        self.waypoints = waypoints
        self.preview = preview
        self.totalDistanceMeters = totalDistanceMeters
    }

    public var body: some View {
        VStack(spacing: 8) {
            TrackPreviewView(
                preview,
                style: .hero,
                tag: "\(waypoints.count) points",
                markers: markers
            )
            .frame(height: 150)

            VStack(spacing: 0) {
                ForEach(waypoints) { waypoint in
                    WaypointRow(
                        waypoint: waypoint,
                        isFirst: waypoint.index == waypoints.first?.index,
                        isLast: waypoint.index == waypoints.last?.index,
                        showsDivider: waypoint.index != waypoints.last?.index
                    )
                }
            }
        }
    }

    /// Middle waypoints pinned on the polyline (start/end already have node
    /// dots). Position = the track point nearest the waypoint's fraction of
    /// total distance — valid as long as the polyline sampling is uniform
    /// (`TrackPreview` downsamples by uniform stride).
    private var markers: [TrackPreviewView.Marker] {
        guard let preview, preview.points.count > 1, totalDistanceMeters > 0 else { return [] }
        let inner = waypoints.dropFirst().dropLast()
        return inner.map { waypoint in
            let fraction = max(0, min(waypoint.distanceAlongMeters / totalDistanceMeters, 1))
            let index = Int((fraction * Double(preview.points.count - 1)).rounded())
            return TrackPreviewView.Marker(
                id: waypoint.index,
                point: preview.points[index],
                label: "\(waypoint.index + 1)"
            )
        }
    }

    /// The marker-placement rule, exposed for unit tests.
    static func markerPointIndex(fraction: Double, pointCount: Int) -> Int {
        Int((max(0, min(fraction, 1)) * Double(pointCount - 1)).rounded())
    }
}

#Preview("Waypoints") {
    ScrollView {
        WaypointsListView(
            waypoints: [
                Waypoint(index: 0, name: "Ottawa Lake trailhead", note: "Start · parking & water", distanceAlongMeters: 0, coordinate: .init(latitude: 42.9, longitude: -88.6)),
                Waypoint(index: 1, name: "Emma Carlin junction", note: "Water · trail crossing", distanceAlongMeters: 14200, coordinate: .init(latitude: 42.9, longitude: -88.5)),
                Waypoint(index: 2, name: "Bald Bluff overlook", note: "Summit · 12% pitch before", distanceAlongMeters: 31600, coordinate: .init(latitude: 42.9, longitude: -88.4)),
                Waypoint(index: 3, name: "Ottawa Lake", note: "Finish", distanceAlongMeters: 62400, coordinate: .init(latitude: 42.9, longitude: -88.6)),
            ],
            preview: .obcSample,
            totalDistanceMeters: 62400
        )
        .padding(20)
    }
    .background(OBCTheme.parchment)
}
