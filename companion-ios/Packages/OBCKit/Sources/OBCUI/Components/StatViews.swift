import SwiftUI

/// One formatted statistic for the strips and grids: a tabular value, an optional small
/// unit, and an uppercase key ("62.4 km / DISTANCE").
public struct OBCStat: Identifiable {
    public let value: String
    public let unit: String?
    public let key: String

    public var id: String { key }

    public init(value: String, unit: String? = nil, key: String) {
        self.value = value
        self.unit = unit
        self.key = key
    }
}

/// The inline stat strip on route and ride detail: equal-width stats in a panel card.
public struct OBCStatStrip: View {
    let stats: [OBCStat]

    public init(_ stats: [OBCStat]) { self.stats = stats }

    public var body: some View {
        // A fixed gutter between columns: equal-flex cells alone let a long value
        // ("20.4 kph") run right up against its neighbour.
        HStack(spacing: 10) {
            ForEach(stats) { stat in
                VStack(alignment: .leading, spacing: 3) {
                    statValue(stat, style: .title3)
                    statKey(stat)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.vertical, 15)
        .padding(.horizontal, 12)
        .background(OBCTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline))
    }
}

/// The two-column stat grid under a full-bleed route card, in hairline-divided cells.
public struct OBCStatGrid: View {
    let stats: [OBCStat]

    public init(_ stats: [OBCStat]) { self.stats = stats }

    public var body: some View {
        let rows = stride(from: 0, to: stats.count, by: 2).map { Array(stats[$0..<min($0 + 2, stats.count)]) }
        VStack(spacing: 1) {
            ForEach(0..<rows.count, id: \.self) { r in
                HStack(spacing: 1) {
                    ForEach(rows[r]) { stat in
                        cell(stat)
                    }
                    if rows[r].count == 1 {
                        OBCTheme.surface.frame(maxWidth: .infinity)
                    }
                }
            }
        }
        .background(OBCTheme.hairline)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.hairline))
    }

    private func cell(_ stat: OBCStat) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            statValue(stat, style: .title2)
            statKey(stat)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(OBCTheme.surface)
    }
}

/// Shared value and key text, used by both stat layouts.
private func statValue(_ stat: OBCStat, style: Font.TextStyle) -> some View {
    (Text(stat.value)
        .font(.obcStat(style))
        .foregroundColor(OBCTheme.ink)
        + Text(stat.unit.map { " \($0)" } ?? "")
        .font(.system(.subheadline, weight: .medium))
        .foregroundColor(OBCTheme.secondary))
        .lineLimit(1)
        .minimumScaleFactor(0.7)
}

private func statKey(_ stat: OBCStat) -> some View {
    OBCEyebrow(stat.key)
}

#Preview("Stats") {
    VStack(spacing: 20) {
        OBCStatStrip([
            OBCStat(value: "62.4", unit: "km", key: "Distance"),
            OBCStat(value: "840", unit: "m", key: "Climb"),
            OBCStat(value: "3:20", key: "Est. time"),
            OBCStat(value: "4", key: "Points"),
        ])
        OBCStatGrid([
            OBCStat(value: "58.2", unit: "km", key: "Distance"),
            OBCStat(value: "2:51", key: "Moving"),
            OBCStat(value: "20.4", unit: "kph", key: "Avg"),
            OBCStat(value: "812", unit: "m", key: "Climb"),
        ])
    }
    .padding()
    .background(OBCTheme.page)
}
