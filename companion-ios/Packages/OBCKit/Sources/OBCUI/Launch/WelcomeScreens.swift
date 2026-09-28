import SwiftUI

struct WelcomeView: View {
    let onStart: () -> Void
    let onBrowse: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .home(name: "OBC"))
                    .padding(.bottom, 32)
                LaunchTitle("Your next ride starts here")
                    .accessibilityIdentifier("onboarding.welcomeTitle")
                    .padding(.bottom, 12)
                LaunchMessage("Send routes to your OBC. Bring your rides home.")
                    .padding(.bottom, 24)
                Text("No account. No subscription. No cloud.")
                    .font(.system(.footnote, weight: .medium))
                    .foregroundStyle(OBCTheme.ink)
                    .multilineTextAlignment(.center)
            }
        } actions: {
            Button("Set up my OBC", action: onStart)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("onboarding.getStarted")
            Button("Browse the library first", action: onBrowse)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("onboarding.browse")
        }
    }
}

struct SwitchOnView: View {
    let onFind: () -> Void
    let onBack: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                DeviceGlyphView(variant: .welcome)
                    .padding(.bottom, 28)
                LaunchTitle("Switch on your OBC")
                    .accessibilityIdentifier("onboarding.switchOnTitle")
                    .padding(.bottom, 12)
                LaunchMessage("Pick a language on your OBC and follow its setup steps.")
                    .padding(.bottom, 22)
                Label {
                    Text("When the QR code appears, scan it with your iPhone camera. Or find your OBC below.")
                        .fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: "qrcode.viewfinder")
                        .font(.system(.title2))
                }
                .font(.system(.subheadline))
                .foregroundStyle(OBCTheme.secondary)
                .frame(maxWidth: 320)
                .lineSpacing(3)
            }
        } actions: {
            Button("Find my OBC", action: onFind)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("pair.start")
            Button("Back", action: onBack)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("onboarding.back")
        }
    }
}

struct BluetoothPermissionView: View {
    let onAllow: () -> Void
    let onBack: () -> Void
    let onBrowse: () -> Void

    var body: some View {
        LaunchScreenScaffold {
            VStack(spacing: 0) {
                BluetoothTile()
                    .padding(.bottom, 32)
                LaunchTitle("Connect with Bluetooth")
                    .accessibilityIdentifier("onboarding.bluetoothTitle")
                    .padding(.bottom, 12)
                LaunchMessage("Bluetooth sends routes, rides and updates between your iPhone and your OBC.")
                    .padding(.bottom, 20)
                LaunchMessage("Keep your OBC nearby. If iOS asks, allow Bluetooth access.")
            }
        } actions: {
            Button("Continue", action: onAllow)
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("onboarding.allowBluetooth")
            Button("Back", action: onBack)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("onboarding.bluetoothBack")
            Button("Browse library", action: onBrowse)
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("onboarding.browse")
        }
    }
}
