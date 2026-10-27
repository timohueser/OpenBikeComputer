import SwiftUI
import OBCDomain

/// Centered glyph, title line, and one action: the single recipe behind every empty
/// and error state. Empty is not broken, so it always points at the one action that
/// fixes it. It never dead-ends and never blames the rider.
public struct OBCEmptyStateView: View {
    /// The glyph treatments the empty and error states use.
    public enum Glyph {
        /// A sketch tile with the zigzag route mark.
        case trackTile
        /// A warning-tinted circle around a system image.
        case warning(systemImage: String)
        /// A sunken circle around a system image.
        case muted(systemImage: String)
    }

    let glyph: Glyph
    let title: String
    let message: String
    var actionTitle: String?
    var actionSystemImage: String?
    var action: () -> Void

    public init(
        glyph: Glyph,
        title: String,
        message: String,
        actionTitle: String? = nil,
        actionSystemImage: String? = nil,
        action: @escaping () -> Void = {}
    ) {
        self.glyph = glyph
        self.title = title
        self.message = message
        self.actionTitle = actionTitle
        self.actionSystemImage = actionSystemImage
        self.action = action
    }

    public var body: some View {
        VStack(spacing: 6) {
            glyphView

            Text(title)
                .font(.system(.title3, weight: .bold))
                .foregroundStyle(OBCTheme.ink)
                .padding(.top, 14)
                .multilineTextAlignment(.center)

            Text(message)
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 240)

            if let actionTitle {
                Button {
                    action()
                } label: {
                    if let actionSystemImage {
                        Label(actionTitle, systemImage: actionSystemImage)
                    } else {
                        Text(actionTitle)
                    }
                }
                .buttonStyle(.obcPrimary(fullWidth: false))
                .padding(.top, 12)
            }
        }
        .padding(.horizontal, 24)
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private var glyphView: some View {
        switch glyph {
        case .trackTile:
            TrackPreviewView(nil, showsChrome: false)
                .frame(width: 112, height: 84)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusCard))
                .accessibilityHidden(true)
        case .warning(let systemImage):
            Image(systemName: systemImage)
                .font(.system(.title, weight: .medium))
                .foregroundStyle(OBCTheme.danger)
                .frame(width: 72, height: 72)
                .background(OBCTheme.danger.opacity(0.1))
                .clipShape(Circle())
        case .muted(let systemImage):
            Image(systemName: systemImage)
                .font(.system(.largeTitle, weight: .medium))
                .foregroundStyle(OBCTheme.secondary)
                .frame(width: 78, height: 78)
                .background(OBCTheme.fill)
                .clipShape(Circle())
        }
    }
}
