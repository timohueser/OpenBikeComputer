import SwiftUI

/// The slim strip below the top bar: a surface panel with the icon in the tone's colour,
/// olive for out of range, danger red for an interrupted sync, and an optional inline
/// action. Reconnection is silent: the banner just disappears.
public struct OBCInlineBanner: View {
    public enum Tone {
        /// Out of range, or informational.
        case notice
        /// Interrupted or failed.
        case warning

        var accent: Color {
            switch self {
            case .notice: OBCTheme.secondary
            case .warning: OBCTheme.danger
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
        tone: Tone = .notice,
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
                .font(.system(.subheadline, weight: .semibold))
                .foregroundStyle(tone.accent)

            Text("\(Text(title).fontWeight(.semibold)) \(message)")
                .font(.system(.footnote))
                .foregroundStyle(OBCTheme.ink)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)

            if let actionTitle {
                Button(action: action) {
                    Text(actionTitle)
                        .font(.system(.footnote, weight: .semibold))
                        .foregroundStyle(OBCTheme.tint)
                        .padding(.horizontal, 6)
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.vertical, 4)
        .padding(.leading, 14)
        .padding(.trailing, actionTitle == nil ? 14 : 8)
        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
    }
}

/// A transient ink capsule with page-coloured text, presented through `.obcToast`.
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
                .font(.system(.footnote, weight: .bold))
            Text(message)
                .font(.system(.footnote, weight: .medium))
                .foregroundStyle(OBCTheme.page)
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
    /// Overlays a transient `OBCToast` at the top edge; it auto-dismisses after
    /// `duration`.
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
    .background(OBCTheme.page)
}
