import SwiftUI

/// Radio off, or the state after the rider denied Bluetooth. Say which switch fixes it, and
/// never trap the rider: the library stays reachable.
struct RadioBlockedView: View {
    let block: LaunchFlowModel.RadioBlock
    let onRetry: () -> Void
    let onBrowseLibrary: () -> Void

    @Environment(\.openURL) private var openURL

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                RadioBlockedGlyph(block: block)
                    .padding(.bottom, 28)

                LaunchTitle(title)
                    .accessibilityIdentifier("radio.title")
                    .padding(.bottom, 8)

                LaunchMessage(message)
            }
        } actions: {
            switch block {
            case .off:
                // iOS has no public link to the Bluetooth switch, so the fix is the rider's and
                // the action re-checks.
                Button("Try again", action: onRetry)
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("radio.tryAgain")
            case .denied:
                Button("Open Settings", action: openSettings)
                    .buttonStyle(.obcPrimary)
                    .accessibilityIdentifier("radio.openSettings")
                Button("Try again", action: onRetry)
                    .buttonStyle(.obcGhost)
                    .accessibilityIdentifier("radio.tryAgain")
            }
            Button("Browse library", action: onBrowseLibrary)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("radio.browseLibrary")
        }
    }

    private var title: String {
        switch block {
        case .off: "Bluetooth is off"
        case .denied: "Allow Bluetooth access"
        }
    }

    private var message: String {
        switch block {
        case .off: "Turn on Bluetooth in Control Center or in Settings ▸ Bluetooth, then try again."
        case .denied: "OBC needs Bluetooth to reach your bike computer. Turn it on for OBC in Settings."
        }
    }

    private func openSettings() {
        #if canImport(UIKit)
        if let url = URL(string: UIApplication.openSettingsURLString) {
            openURL(url)
        }
        #endif
    }
}

#Preview("Bluetooth off") {
    RadioBlockedView(block: .off, onRetry: {}, onBrowseLibrary: {})
}

#Preview("Permission denied") {
    RadioBlockedView(block: .denied, onRetry: {}, onBrowseLibrary: {})
}
