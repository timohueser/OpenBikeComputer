import SwiftUI
import OBCDomain

/// The routes-list panel for a trip: every day drawn on one preview in its palette color, a
/// serif name, and the day-count stat line. The full-width hero preview tells it apart from a
/// ``RouteCard``, which uses the compact side-cell layout.
///
/// The on-device badge is the trip-level ``OnDeviceState`` the caller resolves: a check only
/// when the trip object and every day route are up to date.
public struct TripCard: View {
    let name: String
    let subtitle: String
    /// The trip's date range, when it has a start date.
    let dateLine: String?
    let stages: [MultiTrackPreviewView.Stage]
    let onDevice: OnDeviceState

    public init(
        name: String,
        subtitle: String,
        dateLine: String? = nil,
        stages: [MultiTrackPreviewView.Stage],
        onDevice: OnDeviceState = .notOnDevice
    ) {
        self.name = name
        self.subtitle = subtitle
        self.dateLine = dateLine
        self.stages = stages
        self.onDevice = onDevice
    }

    /// Builds the stat line and the day previews from a trip's totals and its day summaries,
    /// coloring each day by index.
    public init(
        name: String,
        stats: TripStats,
        daySummaries: [RouteSummary],
        dateLine: String? = nil,
        onDevice: OnDeviceState = .notOnDevice
    ) {
        self.init(
            name: name,
            subtitle: OBCFormat.tripSubtitle(
                dayCount: stats.dayCount,
                distanceMeters: stats.distanceMeters,
                elevationGainMeters: stats.elevationGainMeters
            ),
            dateLine: dateLine,
            stages: daySummaries.enumerated().map { index, summary in
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
                if let dateLine {
                    Text(dateLine)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .lineLimit(1)
                        .accessibilityIdentifier("tripCard.dates")
                }
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
                subtitle: "2 days · 141 km · 2,050 m ↑",
                dateLine: "Sat 3 Oct – Sun 4 Oct",
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
