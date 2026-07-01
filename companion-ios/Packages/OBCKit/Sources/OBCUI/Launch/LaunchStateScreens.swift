import SwiftUI

// The launch-side states around the pairing flow: A (bonded, quietly
// reconnecting) and H7/H8 (radio blocked). Dumb views, design copy verbatim.

/// A — bonded launch, connecting. Brief and non-blocking by contract: the flow
/// model caps it (`Timing.connectGrace`) and always resolves to main.
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
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                        .accessibilityIdentifier("launch.connectingTitle")
                    TrailingDots()
                }
                .padding(.bottom, 10)

                Text("This can take a moment when the device wakes from sleep.")
                    .font(.system(size: 13))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 230)
            }
        } actions: {
            brandChip
        }
    }

    /// The OBC wordmark chip anchored at the bottom of the launch state.
    private var brandChip: some View {
        HStack(spacing: 8) {
            Text("OBC")
                .font(.obcMono(size: 11, weight: .bold))
            Text("OpenBikeComputer")
                .font(.system(size: 11))
                .opacity(0.85)
        }
        .foregroundStyle(.white)
        .padding(.vertical, 8)
        .padding(.horizontal, 12)
        .background(OBCTheme.forest, in: RoundedRectangle(cornerRadius: 8))
    }

    /// The design's animated trailing "···".
    private struct TrailingDots: View {
        var body: some View {
            TimelineView(.periodic(from: .now, by: 0.4)) { context in
                let step = Int(context.date.timeIntervalSinceReferenceDate / 0.4) % 3
                HStack(spacing: 2) {
                    ForEach(0..<3, id: \.self) { index in
                        Text("·")
                            .font(.system(size: 15, weight: .bold))
                            .foregroundStyle(OBCTheme.ink)
                            .opacity(index <= step ? 1 : 0.25)
                    }
                }
            }
        }
    }
}

/// H8 (radio off) / the post-denial H7 state. Point to the fix; don't nag —
/// and never trap the rider: the library stays reachable.
struct RadioBlockedView: View {
    let block: LaunchFlowModel.RadioBlock
    let onBrowseLibrary: () -> Void

    @Environment(\.openURL) private var openURL

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 6) {
                Circle()
                    .fill(OBCTheme.parchment3)
                    .frame(width: 78, height: 78)
                    .overlay {
                        BluetoothRune(slashed: true)
                            .stroke(OBCTheme.inkSoft, style: StrokeStyle(lineWidth: 1.9, lineCap: .round, lineJoin: .round))
                            .frame(width: 36, height: 36)
                    }
                    .padding(.bottom, 12)

                Text(title)
                    .font(.obcSerif(size: 20))
                    .foregroundStyle(OBCTheme.ink)
                    .accessibilityIdentifier("radio.title")

                Text(message)
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
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

#Preview("H8 · bluetooth off") {
    RadioBlockedView(block: .off, onBrowseLibrary: {})
}

#Preview("H7 · permission denied") {
    RadioBlockedView(block: .denied, onBrowseLibrary: {})
}
