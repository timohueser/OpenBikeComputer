import SwiftUI

/// Inline banner — the slim tinted strip below the top bar. Amber tone for
/// out-of-range, warning tone for sync-interrupted, with an optional inline
/// action ("Resume"). Reconnection is silent — the banner just disappears.
public struct OBCInlineBanner: View {
    public enum Tone {
        /// Out-of-range / informational (amber).
        case amber
        /// Interrupted / failed (warning red).
        case warning

        var accent: Color {
            switch self {
            case .amber: OBCTheme.amber
            case .warning: OBCTheme.warning
            }
        }

        var iconColor: Color {
            switch self {
            case .amber: OBCTheme.coral
            case .warning: OBCTheme.warning
            }
        }
    }

    let tone: Tone
    let systemImage: String
    /// Leading bold fragment ("Trailhead is out of range.").
    let title: String
    /// Regular continuation ("Showing your last sync.").
    let message: String
    var actionTitle: String?
    var action: () -> Void

    public init(
        tone: Tone = .amber,
        systemImage: String,
        title: String,
        message: String,
        actionTitle: String? = nil,
        action: @escaping () -> Void = {}
    ) {
        self.tone = tone
        self.systemImage = systemImage
        self.title = title
        self.message = message
        self.actionTitle = actionTitle
        self.action = action
    }

    public var body: some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(tone.iconColor)

            (Text(title).fontWeight(.semibold) + Text(" ") + Text(message))
                .font(.system(size: 12.5))
                .foregroundStyle(OBCTheme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)

            if let actionTitle {
                Button(action: action) {
                    Text(actionTitle)
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(OBCTheme.forest)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 13)
        .background(tone.accent.opacity(tone == .amber ? 0.16 : 0.1))
        .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusMedium))
        .overlay(
            RoundedRectangle(cornerRadius: OBCTheme.radiusMedium)
                .strokeBorder(tone.accent.opacity(0.5))
        )
    }
}

/// Toast — the transient ink capsule ("You're up to date…"): ink background,
/// parchment text, amber check. Presented via `.obcToast`.
public struct OBCToast: View {
    let systemImage: String
    let message: String

    public init(systemImage: String = "checkmark", message: String) {
        self.systemImage = systemImage
        self.message = message
    }

    public var body: some View {
        HStack(spacing: 9) {
            Image(systemName: systemImage)
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(OBCTheme.amber)
            Text(message)
                .font(.system(size: 13.5, weight: .medium))
                .foregroundStyle(OBCTheme.parchment)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 12)
        .padding(.horizontal, 14)
        .background(OBCTheme.ink)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .shadow(color: OBCTheme.ink.opacity(0.2), radius: 10, y: 8)
    }
}

public extension View {
    /// Overlays a transient `OBCToast` at the top edge; auto-dismisses after
    /// `duration` (~2s).
    func obcToast(
        isPresented: Binding<Bool>,
        systemImage: String = "checkmark",
        message: String,
        duration: Duration = .seconds(2)
    ) -> some View {
        overlay(alignment: .top) {
            if isPresented.wrappedValue {
                OBCToast(systemImage: systemImage, message: message)
                    .padding(.horizontal, 20)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .task {
                        try? await Task.sleep(for: duration)
                        isPresented.wrappedValue = false
                    }
            }
        }
        .animation(.easeOut(duration: 0.25), value: isPresented.wrappedValue)
    }
}

#Preview("Banners + toast") {
    VStack(spacing: 12) {
        OBCToast(message: "You're up to date — no new rides on Trailhead.")
        OBCInlineBanner(
            systemImage: "wifi.slash",
            title: "Trailhead is out of range.",
            message: "Showing your last sync."
        )
        OBCInlineBanner(
            tone: .warning,
            systemImage: "exclamationmark.triangle",
            title: "Sync interrupted.",
            message: "Got 2 of 5 rides.",
            actionTitle: "Resume"
        )
    }
    .padding(20)
    .background(OBCTheme.parchment)
}
