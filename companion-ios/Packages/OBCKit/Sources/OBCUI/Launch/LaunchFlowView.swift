import SwiftUI

public struct LaunchFlowView<Main: View, Setup: View>: View {
    @Bindable private var model: LaunchFlowModel
    private let main: Main
    private let setup: Setup
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    public init(
        model: LaunchFlowModel, @ViewBuilder setup: () -> Setup, @ViewBuilder main: () -> Main
    ) {
        self.model = model
        self.main = main()
        self.setup = setup()
    }

    public var body: some View {
        ZStack {
            screen.id(phaseKey).transition(.opacity)
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.25), value: phaseKey)
        .task { model.start() }
    }

    @ViewBuilder
    private var screen: some View {
        switch model.phase {
        case .idle:
            OBCTheme.page.ignoresSafeArea()
        case .welcome:
            WelcomeView(onStart: model.showSwitchOn, onBrowse: model.browseLibrary)
        case .pairIntro:
            SwitchOnView(onFind: model.startPairing, onBack: model.showWelcome)
        case .bluetoothPermission:
            BluetoothPermissionView(
                onAllow: model.allowBluetooth, onBack: model.showSwitchOn, onBrowse: model.browseLibrary)
        case .scanning(let devices):
            PairScanningView(devices: devices, onTapDevice: model.confirmPairing, onCancel: model.cancelScanning)
        case .pairing:
            PairingBackdropView(onCancel: model.cancelScanning)
        case .paired(let deviceName):
            PairedView(
                deviceName: deviceName, name: $model.deviceNameDraft, saving: model.savingName,
                error: model.nameSaveError, onSave: model.saveNameAndContinue, onKeepName: model.finishPairing)
        case .pairFailed(let failure):
            PairFailedView(failure: failure, onRetry: model.retryPairing, onHelp: model.showPairingHelp)
        case .radioBlocked(let block):
            RadioBlockedView(block: block, onRetry: model.retryPairing, onBrowseLibrary: model.browseLibrary)
        case .setup:
            setup
        case .main:
            main
        }
    }

    private var phaseKey: String {
        switch model.phase {
        case .idle: "idle"
        case .welcome: "welcome"
        case .pairIntro: "pairIntro"
        case .bluetoothPermission: "bluetoothPermission"
        case .scanning: "scanning"
        case .pairing: "pairing"
        case .paired: "paired"
        case .pairFailed(let failure): "pairFailed.\(failure)"
        case .radioBlocked(let block): "radioBlocked.\(block)"
        case .setup: "setup"
        case .main: "main"
        }
    }
}

extension LaunchFlowView where Setup == EmptyView {
    public init(model: LaunchFlowModel, @ViewBuilder main: () -> Main) {
        self.init(model: model, setup: { EmptyView() }, main: main)
    }
}
