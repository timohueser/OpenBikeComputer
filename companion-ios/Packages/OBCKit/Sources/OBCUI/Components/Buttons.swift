import SwiftUI

/// **Primary / Ghost / Warm / Destructive buttons** (§9, EXT) — the OBC button
/// re-tuned to iOS metrics: 13pt radius, 17pt semibold label, 15/20 padding,
/// full width by default (`.btn` in the design). Disabled = 0.42 opacity.
///
///     Button("Upload to Trailhead") { … }.buttonStyle(.obcPrimary)
///     Button("Save to Planned") { … }.buttonStyle(.obcGhost)
///     Button("Delete route") { … }.buttonStyle(.obcDestructive)
public struct OBCButtonStyle: ButtonStyle {
    public enum Kind {
        /// Forest fill, white label.
        case primary
        /// Transparent, forest label, 1.5pt forest-tinted border.
        case ghost
        /// Coral fill, white label (the pairing CTA).
        case warm
        /// Transparent, warning-red label — always confirmed via sheet.
        case destructive
    }

    let kind: Kind
    /// `.btn` is `width:100%` in the design; pass `false` for inline buttons
    /// (the empty state's sized-to-fit CTA).
    var fullWidth = true

    @Environment(\.isEnabled) private var isEnabled

    public init(kind: Kind, fullWidth: Bool = true) {
        self.kind = kind
        self.fullWidth = fullWidth
    }

    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 17, weight: .semibold))
            .foregroundStyle(foreground)
            .padding(.vertical, 15)
            .padding(.horizontal, 20)
            .frame(maxWidth: fullWidth ? .infinity : nil)
            .background(background(pressed: configuration.isPressed))
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.controlRadius))
            .overlay {
                if kind == .ghost {
                    RoundedRectangle(cornerRadius: OBCTheme.controlRadius)
                        .strokeBorder(OBCTheme.forest.opacity(0.4), lineWidth: 1.5)
                }
            }
            .opacity(isEnabled ? 1 : 0.42)
    }

    private var foreground: Color {
        switch kind {
        case .primary, .warm: .white
        case .ghost: OBCTheme.tint
        case .destructive: OBCTheme.warning
        }
    }

    private func background(pressed: Bool) -> Color {
        switch kind {
        case .primary: pressed ? OBCTheme.forestDeep : OBCTheme.tint
        case .warm: OBCTheme.coral.opacity(pressed ? 0.85 : 1)
        case .ghost, .destructive: OBCTheme.forest.opacity(pressed ? 0.08 : 0)
        }
    }
}

public extension ButtonStyle where Self == OBCButtonStyle {
    static var obcPrimary: OBCButtonStyle { OBCButtonStyle(kind: .primary) }
    static var obcGhost: OBCButtonStyle { OBCButtonStyle(kind: .ghost) }
    static var obcWarm: OBCButtonStyle { OBCButtonStyle(kind: .warm) }
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
        Button("Pair now") {}.buttonStyle(.obcWarm)
        Button("Delete route") {}.buttonStyle(.obcDestructive)
        Button("Disabled") {}.buttonStyle(.obcPrimary).disabled(true)
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
