import SwiftUI
import OBCDomain

/// One day of a trip, on the trip page and in the day editor: DAY N in the day colour, the day's
/// name, the stat line, a note under it, and a bar in the day colour as long as the day's share
/// of the longest day. At accessibility sizes the name moves under DAY N and the lines wrap.
public struct TripDayRow: View {
    let color: Color
    let number: Int
    let title: String?
    let detail: String
    /// How the day reaches its stop or rides a gap, or what the device holds of it.
    let note: String?
    /// The day's distance over the longest day's.
    let fraction: Double
    /// Spoken after the visible lines, such as the day's copy on the device.
    let spokenNote: String?
    let showsDivider: Bool
    let action: () -> Void

    @Environment(\.dynamicTypeSize) private var typeSize

    public init(
        color: Color, number: Int, title: String?, detail: String, note: String? = nil, fraction: Double,
        spokenNote: String? = nil, showsDivider: Bool = true, action: @escaping () -> Void = {}
    ) {
        self.color = color
        self.number = number
        self.title = title
        self.detail = detail
        self.note = note
        self.fraction = fraction
        self.spokenNote = spokenNote
        self.showsDivider = showsDivider
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 3) {
                let layout = typeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: 2))
                    : AnyLayout(HStackLayout(alignment: .firstTextBaseline, spacing: 8))
                layout {
                    Text("DAY \(number)")
                        .font(.system(.caption, weight: .semibold).monospacedDigit())
                        .kerning(1)
                        .foregroundStyle(color)
                    if let title {
                        Text(title)
                            .font(.system(.body, weight: .semibold))
                            .foregroundStyle(OBCTheme.ink)
                            .lineLimit(typeSize.isAccessibilitySize ? 3 : 1)
                    }
                }
                Text(detail)
                    .font(.system(.subheadline).monospacedDigit())
                    .foregroundStyle(OBCTheme.secondary)
                    .lineLimit(typeSize.isAccessibilitySize ? nil : 1)
                    .minimumScaleFactor(0.85)
                if let note {
                    Text(note)
                        .font(.system(.footnote))
                        .foregroundStyle(OBCTheme.secondary)
                        .lineLimit(typeSize.isAccessibilitySize ? nil : 1)
                }
                GeometryReader { geometry in
                    Capsule()
                        .fill(color)
                        .frame(width: max(4, geometry.size.width * min(max(fraction, 0), 1)))
                }
                .frame(height: 4)
                .padding(.top, 6)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 12)
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .overlay(alignment: .bottom) {
                if showsDivider { OBCTheme.hairline.frame(height: 1).padding(.leading, 16) }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spokenLabel)
        .accessibilityAddTraits(.isButton)
    }

    /// "Day 2, to Haslach, 61.3 km, 719 m climb, ~3:06, On Trailhead".
    private var spokenLabel: String {
        let figures = detail.replacingOccurrences(of: " · ", with: ", ").replacingOccurrences(of: "↑", with: "climb")
        return ([title.map { "Day \(number), \($0)" } ?? "Day \(number)", figures, note, spokenNote] as [String?])
            .compactMap { $0 }.joined(separator: ", ")
    }
}

/// The line between two days where the next day starts away from where the day ends: the
/// distance, said to be not ridden, and how the rider travels it. With `onPick` a tap picks how.
public struct TripTransferRow: View {
    let kind: TransferKind?
    let meters: Double
    let onPick: ((TransferKind?) -> Void)?

    @Environment(\.dynamicTypeSize) private var typeSize

    public init(kind: TransferKind?, meters: Double, onPick: ((TransferKind?) -> Void)? = nil) {
        self.kind = kind
        self.meters = meters
        self.onPick = onPick
    }

    public var body: some View {
        if let onPick {
            Menu {
                TransferPicker(kind: kind, onPick: onPick)
            } label: {
                line(picks: true)
            }
            .buttonStyle(.plain)
            .accessibilityHint("Choose how you travel it.")
        } else {
            line(picks: false)
        }
    }

    /// "Train · 34.8 km not ridden", or "Transfer · 34.8 km not ridden" before a pick.
    static func text(kind: TransferKind?, meters: Double) -> String {
        "\(kind?.title ?? "Transfer") · \(OBCFormat.distance(meters: meters)) not ridden"
    }

    /// At accessibility sizes the pick moves under the text, so the text keeps the width.
    private func line(picks: Bool) -> some View {
        let layout = typeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 4))
            : AnyLayout(HStackLayout(spacing: 8))
        return layout {
            HStack(spacing: 8) {
                TransferDash()
                if let kind { Image(systemName: kind.systemImage).font(.system(.caption)) }
                Text(Self.text(kind: kind, meters: meters))
                    .font(.system(.footnote).monospacedDigit())
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if picks {
                HStack(spacing: 3) {
                    if kind == nil { Text("Travel by") }
                    Image(systemName: "chevron.up.chevron.down").font(.system(.caption2, weight: .semibold))
                }
                .font(.system(.footnote, weight: .semibold))
            }
        }
        .foregroundStyle(OBCTheme.secondary)
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .frame(minHeight: picks ? 44 : 32)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Self.text(kind: kind, meters: meters))
    }
}

/// The short dashed rule a transfer line starts with, as the maps draw a transfer.
struct TransferDash: View {
    var body: some View {
        Path { path in
            path.move(to: CGPoint(x: 0, y: 0.75))
            path.addLine(to: CGPoint(x: 22, y: 0.75))
        }
        .stroke(OBCTheme.secondary, style: StrokeStyle(lineWidth: 1.5, dash: [4, 3]))
        .frame(width: 22, height: 1.5)
        .accessibilityHidden(true)
    }
}
