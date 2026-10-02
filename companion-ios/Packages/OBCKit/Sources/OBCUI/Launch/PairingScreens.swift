import SwiftUI
import OBCTransport

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

/// A scan with several nearby OBCs. Their advertised names also appear under the device QR code.
struct PairScanningView: View {
    let devices: [PairingDevice]
    let onTapDevice: (PairingDevice) -> Void
    let onCancel: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                ZStack {
                    if devices.isEmpty { PulsingRings() }
                    BluetoothTile()
                }
                .frame(width: 200, height: 180)
                .padding(.bottom, 12)
                LaunchTitle(devices.isEmpty ? "Looking for your OBC" : "Which OBC is yours?")
                    .accessibilityIdentifier("pair.scanningTitle")
                    .padding(.bottom, 12)
                LaunchMessage(devices.isEmpty
                    ? "Keep your OBC awake and nearby."
                    : "Choose the name shown under the QR code on your OBC.")
                    .padding(.bottom, 24)
                if !devices.isEmpty {
                    OBCGroupedSection {
                        ForEach(devices) { device in
                            OBCListRow(label: device.name, showsChevron: true) { onTapDevice(device) }
                                .accessibilityIdentifier("pair.deviceRow.\(device.id.uuidString)")
                        }
                    }
                }
            }
        } actions: {
            Button("Cancel", action: onCancel)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("pair.cancel")
        }
    }
}

/// The beat while pairing completes. On the real path the iOS pairing alert sits over this and
/// asks for the code the device shows.
struct PairingBackdropView: View {
    var onCancel: () -> Void = {}

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .passkey)
                    .padding(.bottom, 28)
                LaunchTitle("Enter the code the OBC shows")
                    .accessibilityIdentifier("pair.pairingTitle")
                    .padding(.bottom, 12)
                LaunchMessage("Use the iOS pairing window to enter the six-digit code.")
            }
        } actions: {
            Button("Cancel", action: onCancel)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("pair.cancel")
        }
    }
}

struct PairedView: View {
    let deviceName: String
    @Binding var name: String
    var saving = false
    var error: String?
    let onSave: () -> Void
    let onKeepName: () -> Void
    var canSave: Bool { !saving && !DeviceRenaming.normalized(name).isEmpty }

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .home(name: deviceName))
                    .padding(.bottom, 28)
                LaunchTitle("Your OBC is paired")
                    .accessibilityIdentifier("pair.pairedTitle")
                    .padding(.bottom, 12)
                LaunchMessage("Give it a name, or keep \(deviceName). You can change this later in Settings.")
                    .padding(.bottom, 24)
                VStack(alignment: .leading, spacing: 8) {
                    Text("Device name")
                        .font(.system(.subheadline, weight: .semibold))
                        .foregroundStyle(OBCTheme.ink)
                    TextField("Device name", text: $name)
                        .font(.system(.body))
                        .foregroundStyle(OBCTheme.ink)
                        .padding(16)
                        .background(OBCTheme.surface, in: RoundedRectangle(cornerRadius: OBCTheme.radiusPanel))
                        .autocorrectionDisabled()
                        .submitLabel(.continue)
                        .onSubmit { if canSave { onSave() } }
                        .disabled(saving)
                        .accessibilityIdentifier("pairing.name")
                    if let error {
                        Text(error)
                            .font(.system(.footnote))
                            .foregroundStyle(OBCTheme.danger)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("pairing.nameError")
                    }
                }
            }
        } actions: {
            Button(saving ? "Saving name…" : "Continue", action: onSave)
                .buttonStyle(.obcPrimary)
                .disabled(!canSave)
                .accessibilityIdentifier("pairing.saveName")
            Button("Keep \(deviceName)", action: onKeepName)
                .buttonStyle(.obcGhost)
                .disabled(saving)
                .accessibilityIdentifier("pairing.keepName")
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

#Preview("Scanning") {
    PairScanningView(devices: [], onTapDevice: { _ in }, onCancel: {})
}

#Preview("Paired") {
    PairedView(deviceName: "Trailhead", name: .constant("Trailhead"), onSave: {}, onKeepName: {})
}

#Preview("Timeout") {
    PairFailedView(failure: .timeout, onRetry: {}, onHelp: {})
}
