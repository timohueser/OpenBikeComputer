import SwiftUI
import OBCDomain
import OBCTransport
import OBCUI

/// The app's root: the B2 launch gate (bond check → quiet reconnect, or the
/// D1–D5 pairing flow) in front of the main screen. Holds only the two seams —
/// `any DeviceTransport` + `any BondStore` — chosen by the composition root.
struct RootView: View {
    @State private var launchModel: LaunchFlowModel
    private let transport: any DeviceTransport

    init(transport: any DeviceTransport, bondStore: any BondStore) {
        self.transport = transport
        _launchModel = State(initialValue: LaunchFlowModel(transport: transport, bondStore: bondStore))
    }

    var body: some View {
        LaunchFlowView(model: launchModel) {
            MainPlaceholderView(transport: transport)
        }
    }
}

/// Placeholder main screen (the real one is B3). Proves the transport seam end
/// to end — fetches `deviceInfo` once and renders it — and carries the one
/// launch-contract obligation B3 will inherit: out of range / disconnected is
/// the S4 banner over browsable content, **never** an error or a blocker.
struct MainPlaceholderView: View {
    let transport: any DeviceTransport

    @State private var info: DeviceInfo?
    @State private var didFail = false
    @State private var connection: ConnectionState = .connected

    var body: some View {
        VStack(spacing: 10) {
            if connection == .outOfRange || connection == .disconnected {
                OBCInlineBanner(
                    systemImage: "wifi.slash",
                    title: "\(info?.name ?? "Your OBC") is out of range.",
                    message: "Showing your last sync."
                )
                .accessibilityIdentifier("disconnectedBanner")
                .padding(.horizontal, 20)
            }

            Spacer()

            Text("OBC")
                .font(.system(size: 44, weight: .bold, design: .rounded))
                .foregroundStyle(OBCTheme.tint)

            if let info {
                Text(info.name)
                    .font(.headline)
                    .foregroundStyle(OBCTheme.ink)
                Text(info.firmwareVersion)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            } else if didFail {
                Text("Library")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                ProgressView()
            }

            Text("main placeholder · B3 lands here")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("mainPlaceholder")

            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(OBCTheme.parchment)
        .task {
            do {
                info = try await transport.deviceInfo()
            } catch {
                // Unreachable link — the banner tells that story; the (future)
                // library stays browsable.
                didFail = true
            }
        }
        .task {
            for await state in transport.state {
                connection = state
            }
        }
    }
}

#if DEBUG
import OBCMock

#Preview("Bonded (main)") {
    let control = MockControl(scenario: .happyPath)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}

#Preview("First run (pairing)") {
    let control = MockControl(scenario: .noDevice)
    RootView(transport: MockTransport(control: control), bondStore: MockBondStore(control: control))
}
#endif
