import SwiftUI

/// One day on the trip page: the day colour, the day number from its position, the day's name,
/// the date and stat line, and a note under it.
public struct TripDayRow: View {
    let color: Color
    let number: Int
    let title: String?
    let detail: String
    /// How the day reaches its stop or rides a gap: "out and back +0.8 km".
    let note: String?
    let showsDivider: Bool
    let action: () -> Void

    public init(
        color: Color, number: Int, title: String?, detail: String, note: String? = nil, showsDivider: Bool = true,
        action: @escaping () -> Void = {}
    ) {
        self.color = color
        self.number = number
        self.title = title
        self.detail = detail
        self.note = note
        self.showsDivider = showsDivider
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Circle().fill(color).frame(width: 10, height: 10)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("Day \(number)")
                            .font(.obcMono(size: title == nil ? 16 : 12))
                            .foregroundStyle(title == nil ? OBCTheme.ink : OBCTheme.inkFaint)
                        if let title {
                            Text(title)
                                .font(.system(size: 16))
                                .foregroundStyle(OBCTheme.ink)
                                .lineLimit(1)
                        }
                    }
                    Text(detail)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)
                    if let note {
                        Text(note)
                            .font(.obcMono(size: 12))
                            .foregroundStyle(OBCTheme.forest)
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.vertical, 12)
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .overlay(alignment: .bottom) {
                if showsDivider { OBCTheme.screenLine.frame(height: 1).padding(.leading, 38) }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
