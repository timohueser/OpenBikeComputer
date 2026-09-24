import SwiftUI

// The launch-side states around the pairing flow: bonded and quietly reconnecting,
// and radio blocked. Dumb views.

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
                    Text("Connecting to \(deviceName)")
                        .font(.system(.subheadline, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("launch.connectingTitle")
                    TrailingDots()
                }
                .padding(.bottom, 10)

                Text("This can take a moment when the device wakes from sleep.")
                    .font(.system(.footnote))
                    .foregroundStyle(OBCTheme.secondary)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 230)
            }
        } actions: {
            brandChip
        }
    }

    private var brandChip: some View {
        HStack(spacing: 8) {
            Text("OBC")
                .font(.system(.caption2, weight: .semibold).monospacedDigit())
            Text("OpenBikeComputer")
                .font(.system(.caption2))
                .opacity(0.85)
        }
        .foregroundStyle(OBCTheme.onRust)
        .padding(.vertical, 8)
        .padding(.horizontal, 12)
        .background(OBCTheme.rust, in: RoundedRectangle(cornerRadius: 8))
    }

    /// The animated trailing "···".
    private struct TrailingDots: View {
        var body: some View {
            TimelineView(.periodic(from: .now, by: 0.4)) { context in
                let step = Int(context.date.timeIntervalSinceReferenceDate / 0.4) % 3
                HStack(spacing: 2) {
                    ForEach(0..<3, id: \.self) { index in
                        Text("·")
                            .font(.system(.subheadline, weight: .bold))
                            .foregroundStyle(OBCTheme.ink)
                            .opacity(index <= step ? 1 : 0.25)
                    }
                }
            }
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
                Circle()
                    .fill(OBCTheme.danger.opacity(0.1))
                    .frame(width: 88, height: 88)
                    .overlay {
                        BluetoothRune(slashed: true)
                            .stroke(OBCTheme.danger, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                            .frame(width: 40, height: 40)
                    }
                    .padding(.bottom, 24)

                Text("Can't reach \(deviceName)")
                    .font(.system(.title, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("launch.connectFailedTitle")
                    .padding(.bottom, 8)

                Text("It's probably asleep or out of range. Your routes are still here — the app connects on its own once \(deviceName) is nearby.")
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 260)
            }
        } actions: {
            Button("Try again", action: onRetry)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("launch.tryAgain")
            Button("Go to routes", action: onGoToRoutes)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("launch.goToRoutes")
        }
    }
}

/// Radio off, or the state after the rider denied Bluetooth. Point to the fix and
/// never trap the rider: the library stays reachable.
struct RadioBlockedView: View {
    let block: LaunchFlowModel.RadioBlock
    let onBrowseLibrary: () -> Void

    @Environment(\.openURL) private var openURL

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 6) {
                Circle()
                    .fill(OBCTheme.fill)
                    .frame(width: 78, height: 78)
                    .overlay {
                        BluetoothRune(slashed: true)
                            .stroke(OBCTheme.secondary, style: StrokeStyle(lineWidth: 1.9, lineCap: .round, lineJoin: .round))
                            .frame(width: 36, height: 36)
                    }
                    .padding(.bottom, 12)

                Text(title)
                    .font(.system(.title3, weight: .bold))
                    .foregroundStyle(OBCTheme.ink)
                    .accessibilityIdentifier("radio.title")

                Text(message)
                    .font(.system(.subheadline))
                    .foregroundStyle(OBCTheme.secondary)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 230)
            }
        } actions: {
            Button("Open Settings", action: openSettings)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("radio.openSettings")
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
        case .off:
            "Turn on Bluetooth to reach your OBC. Your library is still here to browse."
        case .denied:
            "OBC uses Bluetooth to connect to your bike computer — allow it in Settings. Nothing leaves your phone."
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

#Preview("A · connecting") {
    LaunchConnectingView(deviceName: "Trailhead")
}

#Preview("A-timeout · can't reach") {
    LaunchConnectFailedView(deviceName: "Trailhead", onRetry: {}, onGoToRoutes: {})
}

#Preview("H8 · bluetooth off") {
    RadioBlockedView(block: .off, onBrowseLibrary: {})
}

#Preview("H7 · permission denied") {
    RadioBlockedView(block: .denied, onBrowseLibrary: {})
}
