import SwiftUI
import OBCDomain

/// The time in each zone for heart rate and power, or the one line that says why a ride with
/// those sensors has no zones.
struct RideZonesCard: View {
    let timeline: RideTimeline

    private static let zoneChannels: [RideTimeline.Channel] = [.heartRate, .power]

    var body: some View {
        let zoned = Self.zoneChannels.filter { timeline.channels.contains($0) && timeline.isZoned($0) }
        if zoned.isEmpty {
            if let note = RideZonesCard.missingNote(timeline) {
                RideZonesNote(text: note)
            }
        } else {
            VStack(alignment: .leading, spacing: 14) {
                ForEach(zoned) { channel in
                    VStack(alignment: .leading, spacing: 8) {
                        OBCEyebrow("\(channel.title) zones")
                        RideZoneBar(seconds: timeline.timeInZones(channel) ?? [])
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("zones.\(channel.accessibilityKey)")
                }
                if let note = RideZonesCard.missingNote(timeline) {
                    RideZonesNote(text: note)
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
        }
    }

    /// Names the limits that were not set for the zone channels the ride recorded; nil when
    /// nothing is missing.
    static func missingNote(_ timeline: RideTimeline) -> String? {
        let missing = zoneChannels.filter { timeline.channels.contains($0) && !timeline.isZoned($0) }
        guard !missing.isEmpty else { return nil }
        let names = missing.map { $0 == .heartRate ? "max heart rate" : "FTP" }
        let verb = names.count > 1 ? "were" : "was"
        if missing.count == zoneChannels.filter(timeline.channels.contains).count {
            return "No zones for this ride: \(names.joined(separator: " and ")) \(verb) not set on the device."
        }
        return "No \(missing[0].title.lowercased()) zones: \(names[0]) \(verb) not set on the device."
    }
}

struct RideZonesNote: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(.subheadline))
            .foregroundStyle(OBCTheme.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("zones.none")
    }
}

/// One stacked bar of the time in each zone, Z1 left, and a legend with the minutes.
struct RideZoneBar: View {
    let seconds: [TimeInterval]

    var body: some View {
        let total = seconds.reduce(0, +)
        VStack(alignment: .leading, spacing: 6) {
            GeometryReader { proxy in
                HStack(spacing: 0) {
                    ForEach(Array(seconds.enumerated()), id: \.offset) { zone, time in
                        OBCTheme.zones[zone]
                            .frame(width: total > 0 ? proxy.size.width * time / total : 0)
                    }
                }
            }
            .frame(height: 10)
            .background(OBCTheme.fill)
            .clipShape(Capsule())
            LegendRows {
                ForEach(legend, id: \.zone) { legendItem($0) }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityValue(legend.map { "Zone \($0.zone + 1) \($0.spoken)" }.joined(separator: ", "))
    }

    private var legend: [(zone: Int, shown: String, spoken: String)] {
        seconds.enumerated().map { zone, time in
            let minutes = Int((time / 60).rounded())
            let shown = minutes < 60 ? "\(minutes) min" : String(format: "%d:%02d h", minutes / 60, minutes % 60)
            return (zone, shown, "\(minutes) minutes")
        }
    }

    private func legendItem(_ item: (zone: Int, shown: String, spoken: String)) -> some View {
        HStack(spacing: 4) {
            RoundedRectangle(cornerRadius: 2)
                .fill(OBCTheme.zones[item.zone])
                .frame(width: 8, height: 8)
            Text("Z\(item.zone + 1) \(item.shown)")
                .font(.system(.caption2).monospacedDigit())
                .foregroundStyle(OBCTheme.secondary)
                .lineLimit(1)
        }
    }
}

/// Legend items side by side, left to right, starting a new row only where the next item does
/// not fit, so a larger text size wraps instead of stacking one item per line.
struct LegendRows: Layout {
    var spacing: CGFloat = 12
    var rowSpacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        arrange(subviews, width: proposal.width ?? .infinity).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for (subview, origin) in zip(subviews, arrange(subviews, width: bounds.width).origins) {
            subview.place(at: CGPoint(x: bounds.minX + origin.x, y: bounds.minY + origin.y), proposal: .unspecified)
        }
    }

    private func arrange(_ subviews: Subviews, width: CGFloat) -> (origins: [CGPoint], size: CGSize) {
        var origins: [CGPoint] = []
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0, widest: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + rowSpacing
                rowHeight = 0
            }
            origins.append(CGPoint(x: x, y: y))
            widest = max(widest, x + size.width)
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return (origins, CGSize(width: widest, height: y + rowHeight))
    }
}
