import SwiftUI

// The launch-side states around the pairing flow: bonded and quietly reconnecting,
// the bonded device unreachable, and the radio blocked. Dumb views.

/// Bonded launch, connecting. Brief and non-blocking by contract: the flow model caps
/// it with `Timing.connectGrace` and always resolves to main.
struct LaunchConnectingView: View {
    let deviceName: String

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .home(name: deviceName))
                    .padding(.bottom, 34)

                HStack(spacing: 10) {
                    OBCSpinner()
                        .accessibilityHidden(true)
                    Text("Connecting to \(deviceName)")
                        .font(.system(.headline))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("launch.connectingTitle")
                }
                .padding(.bottom, 8)

                LaunchMessage("This can take a moment when the OBC wakes from sleep.")
            }
        } actions: {
            // The wordmark: the product's name in the device's own type.
            PixelText("OpenBikeComputer", size: .label, color: OBCTheme.secondary)
                .accessibilityHidden(true)
        }
    }
}

/// The bonded device did not answer within the connect grace window: asleep, out of
/// range, or powered off. Never a trap: retry re-enters connecting, or head to the
/// routes, and the background connect keeps listening either way.
struct LaunchConnectFailedView: View {
    let deviceName: String
    let onRetry: () -> Void
    let onGoToRoutes: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                // The device drawn dark and grey: it is there, but not answering.
                DeviceGlyphView(variant: .home(name: deviceName))
                    .grayscale(1)
                    .opacity(0.45)
                    .accessibilityHidden(true)
                    .padding(.bottom, 34)

                LaunchTitle("Can't reach \(deviceName)")
                    .accessibilityIdentifier("launch.connectFailedTitle")
                    .padding(.bottom, 8)

                LaunchMessage(
                    "It is asleep or out of range. The app connects on its own when \(deviceName) is nearby."
                )
            }
        } actions: {
            Button("Try again", action: onRetry)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("launch.tryAgain")
            Button("Open Library", action: onGoToRoutes)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("launch.goToRoutes")
        }
    }
}

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

#Preview("Connecting") {
    LaunchConnectingView(deviceName: "Trailhead")
}

#Preview("Can't reach") {
    LaunchConnectFailedView(deviceName: "Trailhead", onRetry: {}, onGoToRoutes: {})
}

#Preview("Bluetooth off") {
    RadioBlockedView(block: .off, onRetry: {}, onBrowseLibrary: {})
}

#Preview("Permission denied") {
    RadioBlockedView(block: .denied, onRetry: {}, onBrowseLibrary: {})
}
