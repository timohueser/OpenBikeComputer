import SwiftUI

// The pairing screens. Dumb views: copy and callbacks, no transport;
// `LaunchFlowView` binds them to `LaunchFlowModel`.

/// Shared page shape: centred content that scrolls when Dynamic Type outgrows the screen, and
/// bottom-pinned actions on the page base.
struct LaunchScreenScaffold<Content: View, Actions: View>: View {
    @ViewBuilder let content: Content
    @ViewBuilder let actions: Actions

    var body: some View {
        VStack(spacing: 0) {
            GeometryReader { geometry in
                ScrollView {
                    content
                        .padding(.vertical, 20)
                        .frame(maxWidth: .infinity, minHeight: geometry.size.height)
                }
                .scrollBounceBehavior(.basedOnSize)
            }
            VStack(spacing: 10) { actions }
                .padding(.top, 8)
                .padding(.bottom, 14)
        }
        .padding(.horizontal, 24)
        .background(OBCTheme.page.ignoresSafeArea())
    }
}

/// A launch screen's headline.
struct LaunchTitle: View {
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(.title, weight: .bold))
            .foregroundStyle(OBCTheme.ink)
            .multilineTextAlignment(.center)
            .accessibilityAddTraits(.isHeader)
    }
}

/// A launch screen's one line under the headline.
struct LaunchMessage: View {
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text)
            .font(.system(.subheadline))
            .foregroundStyle(OBCTheme.secondary)
            .multilineTextAlignment(.center)
            .lineSpacing(3)
            .frame(maxWidth: 300)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// The pairing prompt: the device's pairing card, three literal steps, then one action.
struct PairIntroView: View {
    let onStart: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .passkey)
                    .padding(.bottom, 28)

                LaunchTitle("Pair your OBC")
                    .accessibilityIdentifier("pair.introTitle")
                    .padding(.bottom, 20)

                VStack(spacing: 14) {
                    step(1, "On the OBC, open **Settings ▸ Connections** and check that Bluetooth is on.")
                    step(2, "Keep the OBC near your phone.")
                    step(3, "Tap it in the list, then enter the code it shows.")
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
                .font(.system(.footnote, weight: .semibold).monospacedDigit())
                .foregroundStyle(OBCTheme.surface)
                .frame(width: 26, height: 26)
                .background(OBCTheme.secondary, in: Circle())
                .obcFixedGeometryType()
            Text(text)
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.ink)
                .lineSpacing(3)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }
}

/// Scanning: the rings, and the found device slides in as a row to tap.
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

                LaunchTitle("Looking for your OBC")
                    .accessibilityIdentifier("pair.scanningTitle")
                    .padding(.bottom, 24)

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
                    .fill(OBCTheme.fill)
                    .frame(width: 36, height: 36)
                    .overlay {
                        BluetoothRune()
                            .stroke(OBCTheme.ink, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                            .frame(width: 18, height: 18)
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(device.advertisedName)
                        .font(.system(.body, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                    Text("Tap to pair")
                        .font(.system(.footnote))
                        .foregroundStyle(OBCTheme.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "chevron.right")
                    .font(.system(.footnote, weight: .semibold))
                    .foregroundStyle(OBCTheme.secondary)
            }
            .padding(.vertical, 14)
            .padding(.horizontal, 16)
            .frame(minHeight: 60)
            .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
            .contentShape(RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("pair.deviceRow")
    }
}

/// The beat while pairing completes. On the real path the iOS pairing alert sits over this and
/// asks for the code the device shows.
struct PairingBackdropView: View {
    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .passkey)
                    .padding(.bottom, 28)
                LaunchTitle("Enter the code the OBC shows")
                    .accessibilityIdentifier("pair.pairingTitle")
            }
        } actions: {
        }
    }
}

/// Paired: the device with its name, and one way forward.
struct PairedView: View {
    let deviceName: String
    let onContinue: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .home(name: deviceName))
                    .padding(.bottom, 34)
                LaunchTitle("Paired with \(deviceName)")
                    .accessibilityIdentifier("pair.pairedTitle")
            }
        } actions: {
            Button("Open Library", action: onContinue)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("pair.goToRoutes")
        }
    }
}

/// Timeout or failure: the reason, what to check, and a clear retry.
struct PairFailedView: View {
    let failure: LaunchFlowModel.PairingFailure
    let onRetry: () -> Void
    let onHelp: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                NoLinkMark(tint: failure == .rejected ? OBCTheme.danger : OBCTheme.secondary)
                    .padding(.bottom, 24)

                LaunchTitle(failure.title)
                    .accessibilityIdentifier("pair.failedTitle")
                    .padding(.bottom, 8)

                LaunchMessage(failure.reason)
                    .accessibilityIdentifier("pair.failedReason")

                // Only a timeout gets the checklist; the rejected copy carries its own recovery.
                if failure == .timeout {
                    VStack(spacing: 10) {
                        checkItem("Bluetooth is on in **Settings ▸ Connections** on the OBC.")
                        checkItem("The OBC is awake and near your phone.")
                    }
                    .frame(maxWidth: 300)
                    .padding(.top, 18)
                }
            }
        } actions: {
            Button("Try again", action: onRetry)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("pair.tryAgain")
            Button("Show the steps", action: onHelp)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("pair.help")
        }
    }

    private func checkItem(_ text: LocalizedStringKey) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("•")
                .font(.system(.subheadline, weight: .bold))
                .foregroundStyle(OBCTheme.secondary)
                .accessibilityHidden(true)
            Text(text)
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.ink)
                .lineSpacing(3)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

#Preview("Prompt") {
    PairIntroView(onStart: {})
}

#Preview("Scanning, found") {
    PairScanningView(discovered: .init(name: "Trailhead"), onTapDevice: {}, onCancel: {})
}

#Preview("Pairing") {
    PairingBackdropView()
}

#Preview("Paired") {
    PairedView(deviceName: "Trailhead", onContinue: {})
}

#Preview("Timeout") {
    PairFailedView(failure: .timeout, onRetry: {}, onHelp: {})
}
