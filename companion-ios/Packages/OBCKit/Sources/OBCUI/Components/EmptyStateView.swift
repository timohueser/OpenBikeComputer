import SwiftUI
import OBCDomain

/// Empty / error layout — centered glyph + serif line + one action. The
/// single recipe behind empty lists, bad files, no results, Bluetooth off,
/// and read errors. "Empty ≠ broken": always point at the one action that
/// fixes it — never a dead end, and never blame the rider.
public struct OBCEmptyStateView: View {
    /// The glyph treatments the empty/error states use.
    public enum Glyph {
        /// A 96pt gridded track tile (20pt radius) with the zigzag route mark.
        case trackTile
        /// A 72pt warning-tinted circle around a system image.
        case warning(systemImage: String)
        /// A 78pt sunken parchment circle around a system image.
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
                .font(.obcSerif(size: 19))
                .foregroundStyle(OBCTheme.ink)
                .padding(.top, 14)
                .multilineTextAlignment(.center)

            Text(message)
                .font(.system(size: 14))
                .foregroundStyle(OBCTheme.inkSoft)
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
                .frame(width: 96, height: 96)
                .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusLarge))
                .overlay(
                    RoundedRectangle(cornerRadius: OBCTheme.radiusLarge).strokeBorder(OBCTheme.line)
                )
        case .warning(let systemImage):
            Image(systemName: systemImage)
                .font(.system(size: 30, weight: .medium))
                .foregroundStyle(OBCTheme.warning)
                .frame(width: 72, height: 72)
                .background(OBCTheme.warning.opacity(0.1))
                .clipShape(Circle())
        case .muted(let systemImage):
            Image(systemName: systemImage)
                .font(.system(size: 32, weight: .medium))
                .foregroundStyle(OBCTheme.inkSoft)
                .frame(width: 78, height: 78)
                .background(OBCTheme.parchment3)
                .clipShape(Circle())
        }
    }
}

#Preview("Empty / error states") {
    ScrollView {
        VStack(spacing: 44) {
            OBCEmptyStateView(
                glyph: .trackTile,
                title: "No planned routes yet",
                message: "Tap + to import a .gpx from Files, or share one from Komoot, Strava, or any app.",
                actionTitle: "Import a route",
                actionSystemImage: "plus"
            )
            OBCEmptyStateView(
                glyph: .warning(systemImage: "exclamationmark.triangle"),
                title: "Couldn't read Trailhead",
                message: "The connection dropped mid-read. Your saved routes are still here.",
                actionTitle: "Retry"
            )
            OBCEmptyStateView(
                glyph: .muted(systemImage: "antenna.radiowaves.left.and.right.slash"),
                title: "Bluetooth is off",
                message: "Turn on Bluetooth to reach Trailhead. Your library is still here to browse.",
                actionTitle: "Open Settings"
            )
        }
        .padding(.vertical, 30)
    }
    .background(OBCTheme.parchment)
}
