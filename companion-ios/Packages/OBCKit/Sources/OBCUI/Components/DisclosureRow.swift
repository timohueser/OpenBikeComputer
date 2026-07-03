import SwiftUI

/// The tappable disclosure row on route detail — the "Waypoints · 4 ›" entry
/// that pushes the waypoints screen. A standalone panel row: 30pt
/// amber-tinted icon tile (9pt radius), label, mono value, chevron.
public struct OBCDisclosureRow: View {
    let systemImage: String
    let label: String
    let value: String?
    let action: () -> Void

    public init(
        systemImage: String,
        label: String,
        value: String? = nil,
        action: @escaping () -> Void = {}
    ) {
        self.systemImage = systemImage
        self.label = label
        self.value = value
        self.action = action
    }

    public var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: systemImage)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(OBCTheme.amber)
                    .frame(width: 30, height: 30)
                    .background(OBCTheme.amber.opacity(0.16))
                    .clipShape(RoundedRectangle(cornerRadius: 9))

                Text(label)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundStyle(OBCTheme.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if let value {
                    Text(value)
                        .font(.obcMono(size: 14))
                        .foregroundStyle(OBCTheme.inkFaint)
                }

                Image(systemName: "chevron.right")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .padding(.leading, 10)
            }
            .padding(.vertical, 15)
            .padding(.horizontal, 16)
            .background(OBCTheme.panel)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .overlay(
                RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line)
            )
        }
        .buttonStyle(.plain)
    }
}

#Preview("Disclosure row") {
    OBCDisclosureRow(systemImage: "mappin.and.ellipse", label: "Waypoints", value: "4")
        .padding(20)
        .background(OBCTheme.parchment)
}
