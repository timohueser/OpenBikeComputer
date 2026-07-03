import SwiftUI

/// One formatted statistic for the strips/grids: a mono value, an optional
/// small unit, and an uppercase key ("62.4 km / DISTANCE").
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

/// The inline stat strip on route/ride detail: equal-width stats in a panel
/// card — 20pt mono value with a 12pt faint unit over a 9.5pt uppercase key.
public struct OBCStatStrip: View {
    let stats: [OBCStat]

    public init(_ stats: [OBCStat]) { self.stats = stats }

    public var body: some View {
        // Fixed gutter: equal-flex cells alone let a long value ("20.4 kph")
        // run right up against its neighbour.
        HStack(spacing: 10) {
            ForEach(stats) { stat in
                VStack(alignment: .leading, spacing: 3) {
                    statValue(stat, size: 20, unitSize: 12)
                    statKey(stat, size: 9.5)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.vertical, 15)
        .padding(.horizontal, 12)
        .background(OBCTheme.panel)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
    }
}

/// The 2-column stat grid under a full-bleed route card — hairline-divided
/// panel cells with a 24pt value.
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
                        OBCTheme.panel.frame(maxWidth: .infinity)
                    }
                }
            }
        }
        .background(OBCTheme.line)
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
    }

    private func cell(_ stat: OBCStat) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            statValue(stat, size: 24, unitSize: 13)
            statKey(stat, size: 10)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(OBCTheme.panel)
    }
}

/// Shared value/key text used by both stat layouts.
private func statValue(_ stat: OBCStat, size: CGFloat, unitSize: CGFloat) -> some View {
    (Text(stat.value)
        .font(.obcMono(size: size, weight: .medium))
        .foregroundColor(OBCTheme.ink)
        + Text(stat.unit.map { " \($0)" } ?? "")
        .font(.obcMono(size: unitSize, weight: .medium))
        .foregroundColor(OBCTheme.inkFaint))
        .lineLimit(1)
        .minimumScaleFactor(0.7)
}

private func statKey(_ stat: OBCStat, size: CGFloat) -> some View {
    Text(stat.key.uppercased())
        .font(.obcMono(size: size, weight: .bold))
        .kerning(1)
        .foregroundStyle(OBCTheme.inkFaint)
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
    .background(OBCTheme.parchment)
}
