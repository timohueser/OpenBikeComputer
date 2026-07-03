import SwiftUI
import OBCDomain

/// Route card — track preview + title + mono stat line. Two variants:
/// - `RouteCard` (compact) — 128pt track cell on the left, dense rows for a
///   big library. The main-screen layout.
/// - `RouteCardFullBleed` — track on top, title + stat grid below (detail-ish
///   feature card).
///
/// Convenience inits take `RouteSummary` (planned) or `RideSummary` (tracked)
/// and format the stat lines via `OBCFormat`.
public struct RouteCard: View {
    let title: String
    let subtitle: String
    let preview: TrackPreview?
    /// The device-copy state — picks the small badge (check = up to date,
    /// refresh = on device but out of date, nothing when not on the device).
    let onDevice: OnDeviceState

    public init(title: String, subtitle: String, preview: TrackPreview?, onDevice: OnDeviceState = .notOnDevice) {
        self.title = title
        self.subtitle = subtitle
        self.preview = preview
        self.onDevice = onDevice
    }

    /// Planned-route row: "62.4 km · 840 m ↑ · 3h 20m".
    public init(route: RouteSummary, onDevice: OnDeviceState = .notOnDevice) {
        self.init(
            title: route.name,
            subtitle: OBCFormat.plannedSubtitle(route),
            preview: route.trackPreview,
            onDevice: onDevice
        )
    }

    /// Tracked-ride row: "Yesterday · 58.2 km · 2:51 · 20.4 kph".
    public init(ride: RideSummary, relativeTo now: Date = Date()) {
        self.init(
            title: ride.name,
            subtitle: OBCFormat.trackedSubtitle(ride, relativeTo: now),
            preview: ride.trackPreview
        )
    }

    public var body: some View {
        HStack(spacing: 0) {
            MapTrackPreviewView(preview, showsChrome: false)
                .frame(width: 128)
                .overlay(alignment: .trailing) { OBCTheme.line.frame(width: 1) }

            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 6) {
                    Text(title)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .lineLimit(1)
                    if onDevice != .notOnDevice { OBCOnDeviceBadge(upToDate: onDevice == .upToDate) }
                }
                Text(subtitle)
                    .font(.obcMono(size: 12))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .lineLimit(1)
                    .minimumScaleFactor(0.85)
            }
            .padding(.vertical, 13)
            .padding(.horizontal, 15)
            .frame(maxWidth: .infinity, minHeight: 96, alignment: .leading)
        }
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusCard).strokeBorder(OBCTheme.line))
        .shadow(color: OBCTheme.ink.opacity(0.05), radius: 3, y: 2)
    }
}

/// The small "on device" badge next to a planned route's title. A forest check
/// = the device's copy is up to date; an amber refresh ring = the device holds
/// this route but the phone's version has moved on (rename, re-import) —
/// uploading again updates it in place. Deliberately quiet: the app only
/// tracks routes to push them, so this is the one device fact it shows.
public struct OBCOnDeviceBadge: View {
    let upToDate: Bool

    public init(upToDate: Bool = true) {
        self.upToDate = upToDate
    }

    public var body: some View {
        Image(systemName: upToDate ? "checkmark.circle.fill" : "arrow.triangle.2.circlepath.circle.fill")
            .font(.system(size: 13, weight: .semibold))
            .foregroundStyle(upToDate ? OBCTheme.forest : OBCTheme.amber)
            .accessibilityLabel(upToDate ? "On device" : "On device, out of date")
            .accessibilityIdentifier(upToDate ? "route.onDeviceBadge" : "route.onDeviceBadge.outdated")
    }
}

/// The full-bleed variant — hero track on top, title + optional stat grid
/// below.
public struct RouteCardFullBleed: View {
    let title: String
    let subtitle: String?
    let preview: TrackPreview?
    let stats: [OBCStat]
    var tag: String?

    public init(
        title: String,
        subtitle: String? = nil,
        preview: TrackPreview?,
        stats: [OBCStat] = [],
        tag: String? = nil
    ) {
        self.title = title
        self.subtitle = subtitle
        self.preview = preview
        self.stats = stats
        self.tag = tag
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            MapTrackPreviewView(preview, style: .hero, tag: tag, showsChrome: false)
                .frame(height: 160)
                .overlay(alignment: .bottom) { OBCTheme.line.frame(height: 1) }

            VStack(alignment: .leading, spacing: 10) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.obcSerif(size: 20))
                        .foregroundStyle(OBCTheme.ink)
                        .lineLimit(1)
                    if let subtitle {
                        Text(subtitle)
                            .font(.obcMono(size: 12))
                            .foregroundStyle(OBCTheme.inkFaint)
                            .lineLimit(1)
                    }
                }
                if !stats.isEmpty {
                    HStack(spacing: 0) {
                        ForEach(stats) { stat in
                            VStack(alignment: .leading, spacing: 3) {
                                (Text(stat.value)
                                    .font(.obcMono(size: 17, weight: .medium))
                                    .foregroundColor(OBCTheme.ink)
                                    + Text(stat.unit.map { " \($0)" } ?? "")
                                    .font(.obcMono(size: 11, weight: .medium))
                                    .foregroundColor(OBCTheme.inkFaint))
                                    .lineLimit(1)
                                Text(stat.key.uppercased())
                                    .font(.obcMono(size: 9, weight: .bold))
                                    .kerning(0.8)
                                    .foregroundStyle(OBCTheme.inkFaint)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
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

#Preview("Route cards") {
    ScrollView {
        VStack(spacing: 12) {
            RouteCard(
                title: "Kettle Moraine Loop",
                subtitle: "62.4 km · 840 m ↑ · 3h 20m",
                preview: .obcSample
            )
            RouteCard(
                title: "Blue Mounds Backroads",
                subtitle: "Fri · 79.0 km · 4:12 · 18.8 kph",
                preview: .obcSample
            )
            RouteCardFullBleed(
                title: "Kettle Moraine Loop",
                subtitle: "Southern Unit · gravel & forest doubletrack",
                preview: .obcSample,
                stats: [
                    OBCStat(value: "62.4", unit: "km", key: "Distance"),
                    OBCStat(value: "840", unit: "m", key: "Climb"),
                    OBCStat(value: "3:20", key: "Est."),
                ],
                tag: "Planned"
            )
        }
        .padding(20)
    }
    .background(OBCTheme.parchment)
}
