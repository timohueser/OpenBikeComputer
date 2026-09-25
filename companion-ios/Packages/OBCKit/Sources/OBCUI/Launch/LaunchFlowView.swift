import SwiftUI
import OBCDomain
import OBCTransport

/// The launch gate: renders whatever `LaunchFlowModel` says, from the connecting
/// state through the pairing flow, and hands over to `main` once the flow lands.
/// Own the model at the composition root and pass the main screen as the trailing
/// closure.
public struct LaunchFlowView<Main: View>: View {
    private let model: LaunchFlowModel
    private let main: Main

    public init(model: LaunchFlowModel, @ViewBuilder main: () -> Main) {
        self.model = model
        self.main = main()
    }

    public var body: some View {
        ZStack {
            // ZStack and a transition, rather than a bare switch, so phase changes
            // cross-fade instead of hard-cutting.
            screen
                .id(phaseKey)
                .transition(.opacity)
        }
        .animation(.easeInOut(duration: 0.25), value: phaseKey)
        .task { model.start() }
    }

    @ViewBuilder
    private var screen: some View {
        switch model.phase {
        case .idle:
            OBCTheme.page.ignoresSafeArea()
        case .connecting(let deviceName):
            LaunchConnectingView(deviceName: deviceName)
        case .connectFailed(let deviceName):
            LaunchConnectFailedView(
                deviceName: deviceName,
                onRetry: { model.retryConnect() },
                onGoToRoutes: { model.browseLibrary() }
            )
        case .pairIntro:
            PairIntroView(onStart: { model.startPairing() })
        case .scanning(let discovered):
            PairScanningView(
                discovered: discovered,
                onTapDevice: { model.confirmPairing() },
                onCancel: { model.cancelScanning() }
            )
        case .pairing:
            PairingBackdropView()
        case .paired(let deviceName):
            PairedView(deviceName: deviceName, onContinue: { model.finishPairing() })
        case .pairFailed(let failure):
            PairFailedView(
                failure: failure,
                onRetry: { model.retryPairing() },
                onHelp: { model.showPairingHelp() }
            )
        case .radioBlocked(let block):
            RadioBlockedView(
                block: block,
                onRetry: { model.retryPairing() },
                onBrowseLibrary: { model.browseLibrary() }
            )
        case .main:
            main
        }
    }

    /// A stable per-screen key: the scanning row appearing must animate inside the
    /// screen, not re-create it.
    private var phaseKey: String {
        switch model.phase {
        case .idle: "idle"
        case .connecting: "connecting"
        case .connectFailed: "connectFailed"
        case .pairIntro: "pairIntro"
        case .scanning: "scanning"
        case .pairing: "pairing"
        case .paired: "paired"
        case .pairFailed(let failure): "pairFailed.\(failure)"
        case .radioBlocked(let block): "radioBlocked.\(block)"
        case .main: "main"
        }
    }
}
