import SwiftUI

/// One formatted statistic for the strips and ledgers: a tabular value, an optional small
/// unit, and an uppercase key ("62.4 km / DISTANCE").
public struct OBCStat: Identifiable, Equatable {
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

/// Plain totals below a ride's title. Large text places each total on its own line.
struct OBCStatSummary: View {
    let stats: [OBCStat]

    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        let layout = typeSize >= .xxxLarge
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 14))
            : AnyLayout(HStackLayout(alignment: .top, spacing: 12))
        layout {
            ForEach(stats) { stat in
                VStack(alignment: .leading, spacing: 4) {
                    Text(stat.key)
                        .font(.system(.subheadline))
                        .foregroundStyle(OBCTheme.secondary)
                    statValue(stat, style: .title2)
                        .minimumScaleFactor(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(stat.key)
                .accessibilityValue([stat.value, stat.unit].compactMap { $0 }.joined(separator: " "))
                .accessibilityIdentifier("summary.\(stat.key)")
            }
        }
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

/// The route ledger, in the device route overview's layout: one row per stat, the caption left
/// and the value right, on a surface card with hairlines between rows. At accessibility text
/// sizes the caption sits over its value.
public struct OBCLedger: View {
    let stats: [OBCStat]

    @Environment(\.dynamicTypeSize) private var typeSize

    public init(_ stats: [OBCStat]) { self.stats = stats }

    public var body: some View {
        VStack(spacing: 0) {
            ForEach(stats) { stat in
                row(stat)
                    .overlay(alignment: .top) {
                        if stat.id != stats.first?.id { OBCTheme.hairline.frame(height: 1) }
                    }
            }
        }
        .padding(.horizontal, 16)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
    }

    @ViewBuilder
    private func row(_ stat: OBCStat) -> some View {
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 4))
            : AnyLayout(HStackLayout(alignment: .firstTextBaseline, spacing: 12))
        layout {
            OBCEyebrow(stat.key)
            if !typeSize.isAccessibilitySize { Spacer(minLength: 0) }
            (Text(stat.value).font(.obcStat(.title3)).foregroundColor(OBCTheme.ink)
                + Text(stat.unit.map { "\u{2009}\($0)" } ?? "")
                .font(.system(.subheadline, weight: .medium))
                .foregroundColor(OBCTheme.secondary))
        }
        .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
        .padding(.vertical, 3)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(stat.key)
        .accessibilityValue([stat.value, stat.unit].compactMap { $0 }.joined(separator: " "))
        .accessibilityIdentifier("ledger.\(stat.key)")
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
