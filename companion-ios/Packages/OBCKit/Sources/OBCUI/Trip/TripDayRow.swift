import SwiftUI

/// One day on the trip page: the day colour, "Day 2 · to Ulrichen", and the date and stat line.
public struct TripDayRow: View {
    let color: Color
    let title: String
    let detail: String
    let showsDivider: Bool
    let action: () -> Void

    public init(color: Color, title: String, detail: String, showsDivider: Bool = true, action: @escaping () -> Void = {}) {
        self.color = color
        self.title = title
        self.detail = detail
        self.showsDivider = showsDivider
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Circle().fill(color).frame(width: 10, height: 10)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.system(size: 16))
                        .foregroundStyle(OBCTheme.ink)
                    Text(detail)
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.inkFaint)
                        .lineLimit(1)
                        .minimumScaleFactor(0.85)
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
