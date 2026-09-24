import SwiftUI

public struct OBCButtonStyle: ButtonStyle {
    public enum Kind {
        /// The one action: amber fill, ink label.
        case primary
        /// Transparent, tint label, hairline border.
        case ghost
        /// Transparent with a danger-red label. Always confirmed through a sheet.
        case destructive
    }

    let kind: Kind
    /// Buttons are full width by default; pass `false` for an inline, sized-to-fit
    /// button.
    var fullWidth = true

    @Environment(\.isEnabled) private var isEnabled

    public init(kind: Kind, fullWidth: Bool = true) {
        self.kind = kind
        self.fullWidth = fullWidth
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(.body, weight: .semibold))
            .foregroundStyle(foreground)
            .padding(.vertical, 8)
            .padding(.horizontal, 20)
            .frame(maxWidth: fullWidth ? .infinity : nil, minHeight: 52)
            .background(background(pressed: configuration.isPressed))
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.controlRadius))
            .overlay {
                if kind == .ghost {
                    RoundedRectangle(cornerRadius: OBCTheme.controlRadius)
                        .strokeBorder(OBCTheme.hairlineStrong, lineWidth: 1.5)
                }
            }
    }

    // A disabled button keeps a readable secondary label on a sunken fill; faintness alone
    // never carries the state.
    private var foreground: Color {
        guard isEnabled else { return OBCTheme.secondary }
        return switch kind {
        case .primary: OBCTheme.onAmber
        case .ghost: OBCTheme.tint
        case .destructive: OBCTheme.danger
        }
    }

    private func background(pressed: Bool) -> Color {
        guard isEnabled else { return OBCTheme.fill }
        return switch kind {
        case .primary: OBCTheme.amber.opacity(pressed ? 0.8 : 1)
        case .ghost, .destructive: OBCTheme.fill.opacity(pressed ? 1 : 0)
        }
    }
}

public extension ButtonStyle where Self == OBCButtonStyle {
    static var obcPrimary: OBCButtonStyle { OBCButtonStyle(kind: .primary) }
    static var obcGhost: OBCButtonStyle { OBCButtonStyle(kind: .ghost) }
    static var obcDestructive: OBCButtonStyle { OBCButtonStyle(kind: .destructive) }

    static func obcPrimary(fullWidth: Bool) -> OBCButtonStyle {
        OBCButtonStyle(kind: .primary, fullWidth: fullWidth)
    }
}

#Preview("Buttons") {
    VStack(spacing: 10) {
        Button {} label: {
            Label("Upload to Trailhead", systemImage: "square.and.arrow.up")
        }
        .buttonStyle(.obcPrimary)
        Button("Save to Planned") {}.buttonStyle(.obcGhost)
        Button("Delete route") {}.buttonStyle(.obcDestructive)
        Button("Disabled") {}.buttonStyle(.obcPrimary).disabled(true)
    }
    .padding(20)
    .background(OBCTheme.page)
}
