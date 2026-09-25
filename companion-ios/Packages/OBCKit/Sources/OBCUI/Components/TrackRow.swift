import SwiftUI
import OBCDomain

/// One row of a library list: the 96 x 72 track sketch, the name at full width and an olive stat
/// line. Routes, trips and rides share it. The on-device chip ends the stat line, or takes its own
/// line under it when the line has no room, so it never shortens the name or the stats. A trip's
/// sketch is a stack of cards with the day count on its corner, as the device shows a trip as a
/// folder with a count. At accessibility text sizes the sketch moves above the text.
public struct TrackRow: View {
    let name: String
    let stats: Text
    /// The stat line as VoiceOver reads it, with the on-device state.
    let statsLabel: String
    /// A second olive line, such as a trip's dates.
    let detail: String?
    let sketch: TrackPreviewView
    let onDevice: OnDeviceState
    /// A trip's day count, which stacks the sketch and badges it; `nil` for a route or a ride.
    let dayCount: Int?
    /// `nil` outside selection; otherwise whether this row is picked.
    let isSelected: Bool?

    @Environment(\.dynamicTypeSize) private var typeSize

    /// Planned route: "18.7 km ▲1,087 m 1:19 h".
    public init(route: RouteSummary, onDevice: OnDeviceState = .notOnDevice, isSelected: Bool? = nil) {
        let stats = RowStats(
            distanceMeters: route.distanceMeters,
            climbMeters: route.elevationGainMeters,
            time: route.estimatedDuration.map(OBCFormat.estimatedClock)
        )
        self.init(
            name: route.name, stats: stats, detail: nil,
            sketch: TrackPreviewView(route.trackPreview, showsChrome: false),
            onDevice: onDevice, dayCount: nil, isSelected: isSelected
        )
    }

    /// Recorded ride: "Yesterday · 58.2 km ▲812 m 2:51 h", in the ride colour.
    public init(ride: RideSummary, relativeTo now: Date = Date()) {
        let stats = RowStats(
            lead: OBCFormat.rideDay(ride.date, relativeTo: now),
            distanceMeters: ride.distanceMeters,
            climbMeters: ride.climbMeters,
            time: "\(OBCFormat.movingTime(ride.movingTime)) h"
        )
        self.init(
            name: ride.name, stats: stats, detail: nil,
            sketch: TrackPreviewView(ride.trackPreview, ink: .ride, showsChrome: false),
            onDevice: .notOnDevice, dayCount: nil, isSelected: nil
        )
    }

    /// Trip: every day on one sketch in alternating day colours, "2 days · 56.5 km ▲381 m", and
    /// the dates when the trip has a start date.
    public init(
        tripName: String,
        stats: TripStats,
        daySummaries: [RouteSummary],
        dateLine: String? = nil,
        onDevice: OnDeviceState = .notOnDevice
    ) {
        let rowStats = RowStats(
            lead: stats.dayCount == 1 ? "1 day" : "\(stats.dayCount) days",
            distanceMeters: stats.distanceMeters,
            climbMeters: stats.elevationGainMeters
        )
        let tracks = daySummaries.enumerated().map { index, day in
            (coordinates: day.trackPreview?.coordinates ?? [], ink: TrackPreviewView.Ink.day(index))
        }
        self.init(
            name: tripName, stats: rowStats, detail: dateLine,
            sketch: TrackPreviewView(tracks: tracks, showsChrome: false),
            onDevice: onDevice, dayCount: stats.dayCount, isSelected: nil
        )
    }

    private init(
        name: String, stats: RowStats, detail: String?, sketch: TrackPreviewView,
        onDevice: OnDeviceState, dayCount: Int?, isSelected: Bool?
    ) {
        self.name = name
        self.stats = stats.text
        self.statsLabel = [stats.label, OnDeviceChip.label(for: onDevice)]
            .compactMap { $0 }
            .joined(separator: ", ")
        self.detail = detail
        self.sketch = sketch
        self.onDevice = onDevice
        self.dayCount = dayCount
        self.isSelected = isSelected
    }

    public var body: some View {
        HStack(spacing: 12) {
            if let isSelected { selectionMark(isSelected) }
            if typeSize.isAccessibilitySize {
                VStack(alignment: .leading, spacing: 10) {
                    sketchCell.frame(height: 120)
                    text
                }
            } else {
                HStack(spacing: 14) {
                    sketchCell.frame(width: 96, height: 72)
                    text
                }
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityAddTraits(isSelected == true ? .isSelected : [])
    }

    @ViewBuilder
    private var sketchCell: some View {
        if let dayCount {
            TripStack(sketch: sketch, dayCount: dayCount).accessibilityHidden(true)
        } else {
            sketch
                .clipShape(RoundedRectangle(cornerRadius: Self.sketchRadius))
                .accessibilityHidden(true)
        }
    }

    static let sketchRadius: CGFloat = 10

    private var text: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(name)
                .font(.system(.body, weight: .semibold))
                .foregroundStyle(OBCTheme.ink)
                .lineLimit(typeSize.isAccessibilitySize ? nil : 2)
            statLine
            if let detail {
                Text(detail)
                    .font(.system(.subheadline).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // The list separator starts under the text, past the sketch.
        .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
    }

    /// The stats, then the chip on the same line when both fit, else the chip under them.
    @ViewBuilder
    private var statLine: some View {
        let styled = stats
            .font(.system(.subheadline).monospacedDigit())
            .foregroundStyle(OBCTheme.secondary)
            .accessibilityLabel(statsLabel)
        if onDevice == .notOnDevice {
            styled
        } else {
            let chip = OnDeviceChip(upToDate: onDevice == .upToDate).accessibilityHidden(true)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) {
                    styled.fixedSize()
                    chip
                }
                VStack(alignment: .leading, spacing: 5) {
                    styled.fixedSize(horizontal: false, vertical: true)
                    chip
                }
            }
        }
    }

    private func selectionMark(_ selected: Bool) -> some View {
        Image(systemName: selected ? "checkmark.circle.fill" : "circle")
            .symbolRenderingMode(.palette)
            .foregroundStyle(selected ? OBCTheme.onAmber : OBCTheme.secondary, OBCTheme.amber)
            .font(.system(.title2))
            .obcFixedGeometryType()
            .accessibilityHidden(true)
    }
}

/// A row's stat line and its spoken form.
private struct RowStats {
    let text: Text
    let label: String

    /// "[lead · ]18.7 km ▲1,087 m[ 1:19 h]".
    init(lead: String? = nil, distanceMeters: Double, climbMeters: Double, time: String? = nil) {
        let distance = OBCFormat.distance(meters: distanceMeters)
        let climb = "\(OBCFormat.climbValue(meters: climbMeters)) m"
        let prefix = lead.map { "\($0) · " } ?? ""
        let suffix = time.map { "\u{2002}\($0)" } ?? ""
        text = Text("\(prefix)\(distance)\u{2002}\(Text.obcClimb(meters: climbMeters))\(suffix)")
        label = [lead, distance, "\(climb) climb", time].compactMap { $0 }.joined(separator: ", ")
    }
}

extension Text {
    /// "▲1,087 m": a climb behind a drawn triangle, never the ↑ glyph. VoiceOver reads the
    /// caller's own label.
    static func obcClimb(meters: Double) -> Text {
        let up = Text(Image(systemName: "arrowtriangle.up.fill")).font(.system(.caption2))
        return Text("\(up)\u{2009}\(OBCFormat.climbValue(meters: meters)) m")
    }
}

/// "ON DEVICE" in the device's font on its title-bar rust: the device holds this route. An amber
/// square in front means the device's copy is out of date, and sending again replaces it.
public struct OnDeviceChip: View {
    let upToDate: Bool

    static let height: CGFloat = 16

    public init(upToDate: Bool = true) {
        self.upToDate = upToDate
    }

    /// The spoken state for a list row, `nil` when the route is not on the device.
    static func label(for state: OnDeviceState) -> String? {
        switch state {
        case .notOnDevice: nil
        case .upToDate: "on device"
        case .outdated: "on device, out of date"
        }
    }

    public var body: some View {
        HStack(spacing: 3) {
            if !upToDate {
                RoundedRectangle(cornerRadius: 1)
                    .fill(OBCTheme.amber)
                    .frame(width: 6, height: 6)
            }
            // Two-thirds scale lands each Terminus pixel on two device pixels at 3x.
            PixelText("ON DEVICE", scale: 2 / 3, color: OBCTheme.onRust)
        }
        .padding(.horizontal, 4)
        .frame(height: Self.height)
        .background(OBCTheme.rust, in: RoundedRectangle(cornerRadius: 4))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(upToDate ? "On device" : "On device, out of date")
        .accessibilityIdentifier(upToDate ? "route.onDeviceBadge" : "route.onDeviceBadge.outdated")
    }
}

public extension View {
    /// Places a row in the screen's one grouped list: the surface fill, rounded at the group's
    /// ends, with hairline separators between rows. The row draws its own padding.
    func obcGroupedRow(first: Bool, last: Bool) -> some View {
        let radius = OBCTheme.radiusCard
        return self
            .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
            .listRowBackground(
                UnevenRoundedRectangle(
                    topLeadingRadius: first ? radius : 0,
                    bottomLeadingRadius: last ? radius : 0,
                    bottomTrailingRadius: last ? radius : 0,
                    topTrailingRadius: first ? radius : 0
                )
                .fill(OBCTheme.surface)
                .padding(.horizontal, 16)
            )
            .listRowSeparator(first ? .hidden : .visible, edges: .top)
            .listRowSeparator(last ? .hidden : .visible, edges: .bottom)
            .listRowSeparatorTint(OBCTheme.hairline)
            .alignmentGuide(.listRowSeparatorTrailing) { $0[.trailing] }
    }
}

/// A trip's sketch as a folder: the sketch on a front card, one card behind it offset up and to
/// the right, and the day count on the back card's corner, clear of the track. It fills the same
/// frame as a route's sketch.
private struct TripStack: View {
    let sketch: TrackPreviewView
    let dayCount: Int

    /// How far the back card sits up and to the right of the front one.
    private static let offset: CGFloat = 6
    private static let shape = RoundedRectangle(cornerRadius: TrackRow.sketchRadius)

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            // A shade darker than the front card, so the stack reads as two sheets.
            Self.shape
                .fill(OBCTheme.sketchGround)
                .overlay(Self.shape.fill(OBCTheme.hairline))
                .overlay(Self.shape.strokeBorder(OBCTheme.hairlineStrong))
                .padding(.leading, Self.offset)
                .padding(.bottom, Self.offset)
            sketch
                .clipShape(Self.shape)
                .overlay(Self.shape.strokeBorder(OBCTheme.surface, lineWidth: 1.5))
                .padding(.trailing, Self.offset)
                .padding(.top, Self.offset)
        }
        .overlay(alignment: .topTrailing) { DayCountBadge(count: dayCount).offset(x: 3, y: -3) }
    }
}

/// The device's folder count: the number in its pixel font on a small ink box.
struct DayCountBadge: View {
    let count: Int

    var body: some View {
        PixelText("\(count)", scale: 2 / 3, color: OBCTheme.surface)
            .padding(.horizontal, 4)
            .frame(minWidth: 16, minHeight: 16)
            .background(OBCTheme.ink, in: RoundedRectangle(cornerRadius: 4))
            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(OBCTheme.surface, lineWidth: 1.5))
            .accessibilityIdentifier("trip.dayCountBadge")
    }
}

/// A row's shape while the library loads: the sketch block, then a name and a stat-line bar.
public struct TrackRowSkeleton: View {
    public init() {}

    public var body: some View {
        HStack(spacing: 14) {
            OBCSkeleton(cornerRadius: 10).frame(width: 96, height: 72)
            VStack(alignment: .leading, spacing: 9) {
                OBCSkeleton().frame(width: 150, height: 15)
                OBCSkeleton().frame(width: 110, height: 11)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 12)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Loading")
    }
}

#if DEBUG
#Preview("Track rows") {
    let route = RouteSummary(
        id: RouteID("kettle"), name: "Kettle Moraine Loop", distanceMeters: 62_400,
        elevationGainMeters: 840, estimatedDuration: 3 * 3600 + 12 * 60, trackPreview: .obcSample
    )
    List {
        TrackRow(route: route).obcGroupedRow(first: true, last: false)
        TrackRow(route: route, onDevice: .upToDate).obcGroupedRow(first: false, last: false)
        TrackRow(route: route, onDevice: .outdated).obcGroupedRow(first: false, last: false)
        TrackRow(
            tripName: "Driftless Weekender", stats: TripStats(distanceMeters: 56_500, elevationGainMeters: 381, dayCount: 3),
            daySummaries: [route, route, route], onDevice: .upToDate
        )
        .obcGroupedRow(first: false, last: false)
        TrackRowSkeleton().obcGroupedRow(first: false, last: true)
    }
    .listStyle(.plain)
    .scrollContentBackground(.hidden)
    .background(OBCTheme.page)
}
#endif
