import SwiftUI

// The D-series pairing screens (design §4). Dumb views: exact design copy +
// callbacks, no transport — `LaunchFlowView` binds them to `LaunchFlowModel`.
// Copy is finalized in the design; match it exactly (issue #241).

/// Shared page shape: centered content, bottom-pinned actions, parchment base.
struct LaunchScreenScaffold<Content: View, Actions: View>: View {
    @ViewBuilder let content: Content
    @ViewBuilder let actions: Actions

    var body: some View {
        VStack(spacing: 0) {
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            VStack(spacing: 10) { actions }
                .padding(.bottom, 14)
        }
        .padding(.horizontal, 24)
        .background(OBCTheme.parchment.ignoresSafeArea())
    }
}

/// D1 — the pairing prompt: two short steps, then one clear action.
struct PairIntroView: View {
    let onStart: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .pairing)
                    .padding(.bottom, 26)

                Text("Let's pair your OBC")
                    .font(.obcSerif(size: 27))
                    .foregroundStyle(OBCTheme.ink)
                    .accessibilityIdentifier("pair.introTitle")
                    .padding(.bottom, 8)

                Text("It only takes a few seconds, and only has to happen once.")
                    .font(.system(size: 15))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .multilineTextAlignment(.center)
                    .padding(.bottom, 24)

                VStack(spacing: 14) {
                    step(1, "On the device, open **Settings ▸ Pair** to put it in pairing mode.")
                    step(2, "Keep it within arm's reach of your phone.")
                }
            }
        } actions: {
            Button("Start pairing", action: onStart)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("pair.start")
        }
    }

    private func step(_ number: Int, _ text: LocalizedStringKey) -> some View {
        HStack(alignment: .top, spacing: 13) {
            Text("\(number)")
                .font(.obcMono(size: 13, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 26, height: 26)
                .background(OBCTheme.forest, in: Circle())
            Text(text)
                .font(.system(size: 15))
                .foregroundStyle(OBCTheme.ink)
                .lineSpacing(3)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// D2 — scanning: pulsing rings; the found-device row slides in.
struct PairScanningView: View {
    let discovered: LaunchFlowModel.DiscoveredDevice?
    let onTapDevice: () -> Void
    let onCancel: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                ZStack {
                    PulsingRings()
                    BluetoothTile()
                }
                .frame(width: 200, height: 200)
                .padding(.bottom, 8)

                Text("Looking for your OBC…")
                    .font(.obcSerif(size: 23))
                    .foregroundStyle(OBCTheme.ink)
                    .accessibilityIdentifier("pair.scanningTitle")
                    .padding(.bottom, 6)

                Text("Make sure the device shows “pairing” on its screen.")
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkFaint)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 240)
                    .padding(.bottom, 22)

                if let discovered {
                    deviceRow(discovered)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(.spring(duration: 0.45), value: discovered)
        } actions: {
            Button("Cancel", action: onCancel)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("pair.cancel")
        }
    }

    private func deviceRow(_ device: LaunchFlowModel.DiscoveredDevice) -> some View {
        Button(action: onTapDevice) {
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 9)
                    .fill(OBCTheme.forest.opacity(0.12))
                    .frame(width: 36, height: 36)
                    .overlay {
                        BluetoothRune()
                            .stroke(OBCTheme.forest, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                            .frame(width: 18, height: 18)
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(device.advertisedName)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                    Text("Strong signal · tap to pair")
                        .font(.obcMono(size: 12))
                        .foregroundStyle(OBCTheme.forest)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                OBCSpinner()
                    .scaleEffect(0.8)
            }
            .padding(.vertical, 14)
            .padding(.horizontal, 16)
            .background(OBCTheme.panel)
            .clipShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .overlay(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel).strokeBorder(OBCTheme.line))
            .shadow(color: OBCTheme.ink.opacity(0.05), radius: 3, y: 2)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("pair.deviceRow")
    }
}

/// D3 — the backdrop while pairing completes. On the real path the iOS system
/// pairing alert sits over this (system blue — intentionally not ours, see
/// `OBCSystemPairing`); the app draws only the quiet stage behind it.
struct PairingBackdropView: View {
    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 18) {
                BluetoothTile()
                Text("Pairing…")
                    .font(.obcSerif(size: 23))
                    .foregroundStyle(OBCTheme.ink)
                    .accessibilityIdentifier("pair.pairingTitle")
            }
            .opacity(0.5)
        } actions: {
        }
    }
}

/// D4 — paired: confirm, name shown, one way forward.
struct PairedView: View {
    let deviceName: String
    let onContinue: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                Circle()
                    .fill(OBCTheme.forest)
                    .frame(width: 96, height: 96)
                    .background(Circle().fill(OBCTheme.forest.opacity(0.12)).padding(-10))
                    .overlay {
                        Image(systemName: "checkmark")
                            .font(.system(size: 40, weight: .semibold))
                            .foregroundStyle(.white)
                    }
                    .padding(.bottom, 26)

                Text("Paired with \(deviceName)")
                    .font(.obcSerif(size: 27))
                    .foregroundStyle(OBCTheme.ink)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("pair.pairedTitle")
                    .padding(.bottom, 8)

                Text("Your routes and rides stay between this phone and the device. No account, no cloud.")
                    .font(.system(size: 15))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 250)
            }
        } actions: {
            Button("Go to routes", action: onContinue)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("pair.goToRoutes")
        }
    }
}

/// D5 — timeout / failure: not alarming. Reason + fixes + a clear retry.
struct PairFailedView: View {
    let failure: LaunchFlowModel.PairingFailure
    let onRetry: () -> Void
    let onHelp: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                Circle()
                    .fill(OBCTheme.warning.opacity(0.1))
                    .frame(width: 88, height: 88)
                    .overlay {
                        BluetoothRune(slashed: true)
                            .stroke(OBCTheme.warning, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                            .frame(width: 40, height: 40)
                    }
                    .padding(.bottom, 24)

                Text(failure.title)
                    .font(.obcSerif(size: 25))
                    .foregroundStyle(OBCTheme.ink)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("pair.failedTitle")
                    .padding(.bottom, 8)

                Text(failure.reason)
                    .font(.system(size: 14))
                    .foregroundStyle(OBCTheme.inkSoft)
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
                    .frame(maxWidth: 250)
                    .padding(.bottom, failure == .timeout ? 20 : 0)
                    .accessibilityIdentifier("pair.failedReason")

                // The scan-recovery hints only make sense for a timeout; the
                // `.rejected` combined copy carries its own recovery inline
                // (retry the passkey / Forget phone on the device), so no hint
                // rows there — no dead rows (#461).
                if failure == .timeout {
                    VStack(spacing: 10) {
                        checkItem("The device is showing **“pairing”** on its screen.")
                        checkItem("It's a few metres away, not asleep.")
                    }
                }
            }
        } actions: {
            Button("Try again", action: onRetry)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("pair.tryAgain")
            Button("Pairing help", action: onHelp)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("pair.help")
        }
    }

    private func checkItem(_ text: LocalizedStringKey) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("▸")
                .font(.system(size: 14))
                .foregroundStyle(OBCTheme.amber)
            Text(text)
                .font(.system(size: 14))
                .foregroundStyle(OBCTheme.ink)
                .lineSpacing(3)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

#Preview("D1 · prompt") {
    PairIntroView(onStart: {})
}

#Preview("D2 · scanning (found)") {
    PairScanningView(
        discovered: .init(name: "Trailhead"),
        onTapDevice: {},
        onCancel: {}
    )
}

#Preview("D3 · pairing") {
    PairingBackdropView()
}

#Preview("D4 · paired") {
    PairedView(deviceName: "Trailhead", onContinue: {})
}

#Preview("D5 · timeout") {
    PairFailedView(failure: .timeout, onRetry: {}, onHelp: {})
}
