import SwiftUI
import OBCDomain

/// **Trip Card** (TR6) — the routes-list panel for a *trip*: every stage drawn
/// on one multi-stage preview in its palette color, a serif name, and the
/// `N stages · km · ↑m` stat line. Visually distinct from a ``RouteCard`` by
/// its **full-width hero preview** (route cards use the compact side-cell
/// layout) — the multi-color stage map is the "group of routes" signal. All
/// within existing `OBCTheme` tokens (no new colors — repo rule).
///
/// (An earlier cut added a stacked-cards deck edge behind the panel; the owner
/// cut it 2026-07-13 — it read as noise and broke the list's card rhythm.)
///
/// The on-device badge is the trip-level ``OnDeviceState`` the caller resolves
/// (check only when the trip object *and* every stage are up to date).
public struct TripCard: View {
    let name: String
    let subtitle: String
    let stages: [MultiTrackPreviewView.Stage]
    let onDevice: OnDeviceState

    public init(
        name: String,
        subtitle: String,
        stages: [MultiTrackPreviewView.Stage],
        onDevice: OnDeviceState = .notOnDevice
    ) {
        self.name = name
        self.subtitle = subtitle
        self.stages = stages
        self.onDevice = onDevice
    }

    /// Convenience: build the stat line + stage previews from a trip's summed
    /// stats and its member summaries (palette color by stage index).
    public init(
        name: String,
        stats: TripStats,
        stageSummaries: [RouteSummary],
        onDevice: OnDeviceState = .notOnDevice
    ) {
        self.init(
            name: name,
            subtitle: OBCFormat.tripSubtitle(
                stageCount: stats.stageCount,
                distanceMeters: stats.distanceMeters,
                elevationGainMeters: stats.elevationGainMeters
            ),
            stages: stageSummaries.enumerated().map { index, summary in
                MultiTrackPreviewView.Stage(
                    coordinates: summary.trackPreview?.coordinates ?? [],
                    color: OBCTheme.stageColor(index: index)
                )
            },
            onDevice: onDevice
        )
    }

    public var body: some View {
        card
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("tripCard")
    }

    private var card: some View {
        VStack(alignment: .leading, spacing: 0) {
            MultiTrackPreviewView(stages: stages, showsChrome: false)
                .frame(height: 150)
                .overlay(alignment: .bottom) { OBCTheme.line.frame(height: 1) }

            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Text(name)
                        .font(.obcSerif(size: 19))
                        .foregroundStyle(OBCTheme.ink)
                        .lineLimit(1)
                    if onDevice != .notOnDevice { OBCOnDeviceBadge(upToDate: onDevice == .upToDate) }
                }
                Text(subtitle)
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
                    .accessibilityIdentifier("tripCard.stats")
            }
            .padding(15)
        }
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusCard).strokeBorder(OBCTheme.line))
        .shadow(color: OBCTheme.ink.opacity(0.05), radius: 3, y: 2)
    }
}

#if DEBUG
#Preview("Trip card") {
    let a = TrackPreview.normalizing([
        .init(latitude: 42.90, longitude: -88.52), .init(latitude: 42.93, longitude: -88.49),
        .init(latitude: 42.94, longitude: -88.46),
    ]).coordinates
    let b = TrackPreview.normalizing([
        .init(latitude: 42.86, longitude: -88.40), .init(latitude: 42.84, longitude: -88.36),
        .init(latitude: 42.82, longitude: -88.34),
    ]).coordinates
    return ScrollView {
        VStack(spacing: 14) {
            TripCard(
                name: "Driftless Weekender",
                subtitle: "2 stages · 141 km · 2,050 m ↑",
                stages: [
                    .init(coordinates: a, color: OBCTheme.stageColor(index: 0)),
                    .init(coordinates: b, color: OBCTheme.stageColor(index: 1)),
                ],
                onDevice: .upToDate
            )
        }
        .padding(20)
    }
    .background(OBCTheme.parchment)
    .environment(\.obcIsOnline, false)
}
#endif
