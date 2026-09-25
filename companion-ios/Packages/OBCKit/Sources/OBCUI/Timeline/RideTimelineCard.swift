import SwiftUI
import OBCDomain

/// The ride's channels as stacked strips on one distance axis, with one cursor through them all.
/// A sideways drag or a tap on a strip moves the cursor; a tap on a strip's label opens that
/// channel's detail. Before the first scrub each strip reads its average.
struct RideTimelineCard: View {
    let timeline: RideTimeline
    @Binding var cursor: Double?
    /// The ride's photos on the elevation strip, each from 0 at the start to 1 at the end.
    var photoTicks: [Double] = []
    let onOpen: (RideTimeline.Channel) -> Void

    /// Every strip's plot spans the same width, so one set of columns serves them all.
    @State private var plotWidth: CGFloat = 0
    @State private var plots: [RideTimeline.Channel: RideTimeline.Plot] = [:]

    @Environment(\.dynamicTypeSize) private var typeSize
    @ScaledMetric(relativeTo: .caption) private var labelWidth: CGFloat = 60
    @ScaledMetric(relativeTo: .subheadline) private var valueWidth: CGFloat = 76

    private var columns: Int { timelineColumns(width: plotWidth) }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                OBCEyebrow("Timeline")
                Spacer()
                Text(cursor.map { "KM \(OBCFormat.distanceValue(meters: $0))" } ?? "AVG")
                    .font(.system(.caption, weight: .semibold).monospacedDigit())
                    .kerning(1)
                    .foregroundStyle(OBCTheme.secondary)
            }
            .padding(.bottom, 4)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Timeline position")
            .accessibilityValue(cursor.map { "Kilometre \(OBCFormat.distanceValue(meters: $0))" } ?? "Averages")
            .accessibilityAdjustableAction { direction in
                let step = timeline.length / 20
                let now = cursor ?? (direction == .increment ? -step : timeline.length + step)
                cursor = min(max(now + (direction == .increment ? step : -step), 0), timeline.length)
            }
            .accessibilityIdentifier("timeline.position")
            ForEach(timeline.channels) { channel in
                strip(channel)
                    .overlay(alignment: .top) {
                        if channel != timeline.channels.first { OBCTheme.hairline.frame(height: 1) }
                    }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        .sensoryFeedback(.selection, trigger: cursor.map { Int($0 / max(timeline.length, 1) * 20) })
        .onChange(of: columns, initial: true) { _, columns in
            guard plotWidth > 0 else { return }
            plots = Dictionary(uniqueKeysWithValues: timeline.channels.map { ($0, timeline.plot($0, columns: columns)) })
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("detail.timeline")
    }

    @ViewBuilder
    private func strip(_ channel: RideTimeline.Channel) -> some View {
        let chart = RideChannelChart(
            timeline: timeline, channel: channel, plot: plots[channel] ?? .empty,
            ticks: channel == .elevation ? photoTicks : [], style: .strip, cursor: cursor
        )
        .frame(height: channel.stripHeight)
        .frame(maxWidth: .infinity)
        .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { plotWidth = $0 }
        .modifier(TimelineScrub(length: timeline.length, cursor: $cursor))
        .accessibilityHidden(true)

        if typeSize.isAccessibilitySize {
            VStack(spacing: 4) {
                HStack(alignment: .firstTextBaseline) {
                    label(channel)
                    Spacer(minLength: 8)
                    value(channel)
                }
                chart
            }
            .padding(.vertical, 8)
        } else {
            HStack(spacing: 8) {
                label(channel)
                    .frame(width: labelWidth, alignment: .leading)
                chart
                value(channel)
                    .frame(width: valueWidth, alignment: .trailing)
            }
            .padding(.vertical, 5)
        }
    }

    private func label(_ channel: RideTimeline.Channel) -> some View {
        Button { onOpen(channel) } label: {
            Text(channel.stripLabel.uppercased())
                .font(.system(.caption2, weight: .semibold))
                .kerning(0.8)
                .foregroundStyle(OBCTheme.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(channel.title)
        .accessibilityValue(spokenValue(channel))
        .accessibilityHint("Opens the \(channel.title.lowercased()) detail")
        .accessibilityIdentifier("timeline.\(channel.accessibilityKey)")
    }

    private func value(_ channel: RideTimeline.Channel) -> some View {
        let shown = shownValue(channel)
        return (Text(shown.map(channel.format) ?? "—")
            .font(.obcStat(.subheadline))
            .foregroundColor(shown.map { timeline.color(channel, value: $0) } ?? OBCTheme.ink)
            + Text("\u{2009}\(channel.unit)")
            .font(.system(.caption2, weight: .medium))
            .foregroundColor(OBCTheme.secondary))
            .lineLimit(1)
            .minimumScaleFactor(0.7)
            .accessibilityHidden(true)
    }

    /// The plotted value under the cursor, or the ride's average before the first scrub.
    private func shownValue(_ channel: RideTimeline.Channel) -> Double? {
        guard let cursor else { return timeline.stats(channel)?.mean }
        guard let plot = plots[channel], !plot.values.isEmpty else { return nil }
        return plot.values[timeline.column(at: cursor, columns: plot.values.count)]
    }

    private func spokenValue(_ channel: RideTimeline.Channel) -> String {
        guard let value = shownValue(channel) else { return "No value" }
        let reading = "\(channel.format(value)) \(channel.unit)"
        let zone = timeline.zone(channel, value: value).map { ", zone \($0 + 1)" } ?? ""
        guard let cursor else { return "Average \(reading)\(zone)" }
        return "\(reading)\(zone) at kilometre \(OBCFormat.distanceValue(meters: cursor))"
    }
}

extension RideTimeline.Channel {
    /// The channel's accessibility identifier suffix.
    var accessibilityKey: String {
        switch self {
        case .elevation: "elevation"
        case .speed: "speed"
        case .heartRate: "heartRate"
        case .power: "power"
        case .cadence: "cadence"
        }
    }
}
