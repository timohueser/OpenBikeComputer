import SwiftUI
import OBCDomain
#if canImport(MapKit)
import MapKit
#endif

/// The full-screen **interactive** track map (#294) — default MapKit pinch /
/// zoom / pan over the same halo + stroke polyline and start/end dots the
/// previews draw, framed to the track's bounds on open. Presented as a
/// `fullScreenCover` from the route/ride detail hero.
///
/// Only reached when there's real geometry and a network path (the detail hero
/// only offers the tap when online), so there's no offline/blank-map state to
/// draw here — the grid preview stays put when offline.
///
/// **Trip mode** additionally makes each stage polyline tappable: a tap within
/// reach of a stage raises a small callout card — the stage's palette chip,
/// name, and stat line, with an **Open route** hand-off to that stage's detail
/// page. A tap on empty map dismisses it.
public struct TrackMapView: View {
    private let coordinates: [Coordinate]
    private let waypoints: [Waypoint]
    /// Multi-stage mode (a trip page's hero, TR6): each stage stroked in its
    /// palette color instead of the single accent track. Empty = single-track.
    private let stages: [MultiTrackPreviewView.Stage]
    /// Trip mode's per-stage summaries (parallel to `stages`, ride order) — the
    /// callout's name + stat line. Empty = no callout (segments aren't tappable).
    private let stageSummaries: [RouteSummary]
    /// The callout's **Open route** hand-off — the caller closes this cover and
    /// pushes the stage's detail page. `nil` hides the button.
    private let onOpenStage: ((RouteSummary) -> Void)?
    private let title: String
    private let onClose: () -> Void

    /// The stage whose callout is up (an index into `stages`), or `nil`.
    @State private var selectedStage: Int?

    public init(
        coordinates: [Coordinate],
        waypoints: [Waypoint] = [],
        title: String,
        onClose: @escaping () -> Void
    ) {
        self.coordinates = coordinates
        self.waypoints = waypoints
        self.stages = []
        self.stageSummaries = []
        self.onOpenStage = nil
        self.title = title
        self.onClose = onClose
    }

    /// The trip variant: every stage on one interactive map, in its palette
    /// color — the full-screen sibling of ``MultiTrackPreviewView``. Pass the
    /// stage summaries (same order) to make the segments tappable.
    public init(
        stages: [MultiTrackPreviewView.Stage],
        stageSummaries: [RouteSummary] = [],
        title: String,
        onClose: @escaping () -> Void,
        onOpenStage: ((RouteSummary) -> Void)? = nil
    ) {
        self.coordinates = stages.flatMap(\.coordinates)
        self.waypoints = []
        self.stages = stages
        self.stageSummaries = stageSummaries
        self.onOpenStage = onOpenStage
        self.title = title
        self.onClose = onClose
    }

    public var body: some View {
        NavigationStack {
            mapBody
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle(title)
                #if os(iOS)
                .navigationBarTitleDisplayMode(.inline)
                #endif
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done", action: onClose)
                            .fontWeight(.semibold)
                    }
                }
                .accessibilityIdentifier("trackMap.screen")
        }
        .tint(OBCTheme.tint)
    }

    @ViewBuilder
    private var mapBody: some View {
        #if canImport(MapKit)
        MapReader { proxy in
            Map(initialPosition: .region(MapGeometry.boundingRegion(for: coordinates, pad: 1.4))) {
                if stages.isEmpty {
                    TrackMapContent(coordinates: coordinates, dotRadius: 7, waypoints: waypoints)
                } else {
                    ForEach(Array(stages.enumerated()), id: \.offset) { index, stage in
                        let coords = MapGeometry.clLocations(stage.coordinates)
                        MapPolyline(coordinates: coords)
                            .stroke(
                                OBCTheme.trackHalo,
                                style: StrokeStyle(lineWidth: 7, lineCap: .round, lineJoin: .round))
                        MapPolyline(coordinates: coords)
                            .stroke(
                                stage.color,
                                style: StrokeStyle(
                                    // The selected stage reads picked without any
                                    // new color: its own stroke, one step bolder.
                                    lineWidth: index == selectedStage ? 5 : 3.5,
                                    lineCap: .round, lineJoin: .round))
                    }
                }
            }
            .mapControls {
                MapCompass()
                MapScaleView()
            }
            // Always the light tile set — see `MapTrackPreviewView`'s note. The
            // rest of the app doesn't lean on dark mode either (OBCTheme is a
            // fixed light palette); this just keeps the one system-styled surface
            // consistent with it instead of flipping to Maps' dark tiles.
            .preferredColorScheme(.light)
            .onTapGesture { point in
                guard !stages.isEmpty, !stageSummaries.isEmpty else { return }
                withAnimation(.snappy(duration: 0.22)) {
                    selectedStage = stageIndex(at: point, proxy: proxy)
                }
            }
        }
        .overlay(alignment: .bottom) {
            if let index = selectedStage, let summary = stageSummary(index) {
                stageCallout(index: index, summary: summary)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
            }
        }
        #else
        // No MapKit (host build) → nothing interactive to show.
        Color(OBCTheme.parchment)
        #endif
    }

    private func stageSummary(_ index: Int) -> RouteSummary? {
        stageSummaries.indices.contains(index) ? stageSummaries[index] : nil
    }

    /// The stage-tap callout: palette chip + stage number, the route's name and
    /// the standard planned stat line ("47.2 km · 620 m ↑ · 2h 30m"), and the
    /// Open-route hand-off. A floating panel in the app's card chrome.
    private func stageCallout(index: Int, summary: RouteSummary) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Circle()
                        .fill(stages[index].color)
                        .frame(width: 9, height: 9)
                    Text("STAGE \(index + 1)")
                        .font(.obcMono(size: 11))
                        .foregroundStyle(OBCTheme.inkFaint)
                }
                Text(summary.name)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(OBCTheme.ink)
                    .lineLimit(1)
                Text(OBCFormat.plannedSubtitle(summary))
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
            }
            Spacer(minLength: 8)
            if let onOpenStage {
                Button {
                    onOpenStage(summary)
                } label: {
                    HStack(spacing: 4) {
                        Text("Open route")
                        Image(systemName: "chevron.right")
                            .font(.system(size: 11, weight: .semibold))
                    }
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 9)
                    .background(
                        Capsule().fill(OBCTheme.forest)
                    )
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("trackMap.stageCallout.open")
            }
        }
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: OBCTheme.radiusPanel)
                .fill(OBCTheme.panel)
                .shadow(color: .black.opacity(0.18), radius: 14, y: 4)
        )
        .overlay(
            RoundedRectangle(cornerRadius: OBCTheme.radiusPanel)
                .stroke(OBCTheme.line, lineWidth: 1)
        )
        .padding(.horizontal, 16)
        .padding(.bottom, 18)
        .accessibilityIdentifier("trackMap.stageCallout")
    }

    #if canImport(MapKit)
    /// The stage polyline nearest the tap, within a finger-sized reach (28 pt),
    /// or `nil` (a tap on empty map — dismiss). Each stage's track is projected
    /// to screen points through the live `MapProxy` (subsampled — a callout hit
    /// doesn't need every vertex) and measured point-to-segment.
    private func stageIndex(at point: CGPoint, proxy: MapProxy) -> Int? {
        var best: (index: Int, distance: CGFloat)?
        for (index, stage) in stages.enumerated() {
            let coords = stage.coordinates
            guard coords.count > 1 else { continue }
            let step = max(1, coords.count / 240)
            var screen: [CGPoint] = []
            screen.reserveCapacity(coords.count / step + 2)
            var i = 0
            while i < coords.count {
                if let p = proxy.convert(
                    CLLocationCoordinate2D(latitude: coords[i].latitude, longitude: coords[i].longitude),
                    to: .local)
                {
                    screen.append(p)
                }
                i += step
            }
            // The subsample stride can skip the endpoint — a tap near the finish
            // must still hit.
            if let last = coords.last, coords.count % step != 1 {
                if let p = proxy.convert(
                    CLLocationCoordinate2D(latitude: last.latitude, longitude: last.longitude),
                    to: .local)
                {
                    screen.append(p)
                }
            }
            guard screen.count > 1 else { continue }
            for j in 1..<screen.count {
                let d = Self.distance(from: point, toSegmentFrom: screen[j - 1], to: screen[j])
                if d < (best?.distance ?? .infinity) {
                    best = (index, d)
                }
            }
        }
        guard let best, best.distance <= 28 else { return nil }
        return best.index
    }

    /// Euclidean distance from `p` to the segment `a`–`b`.
    static func distance(from p: CGPoint, toSegmentFrom a: CGPoint, to b: CGPoint) -> CGFloat {
        let ab = CGPoint(x: b.x - a.x, y: b.y - a.y)
        let ap = CGPoint(x: p.x - a.x, y: p.y - a.y)
        let lengthSquared = ab.x * ab.x + ab.y * ab.y
        guard lengthSquared > 0 else { return hypot(ap.x, ap.y) }
        let t = max(0, min(1, (ap.x * ab.x + ap.y * ab.y) / lengthSquared))
        let closest = CGPoint(x: a.x + t * ab.x, y: a.y + t * ab.y)
        return hypot(p.x - closest.x, p.y - closest.y)
    }
    #endif
}

#if DEBUG
#Preview("Track map") {
    TrackMapView(
        coordinates: TrackPreview.obcSample.coordinates,
        title: "Kettle Moraine Loop",
        onClose: {}
    )
}
#endif
