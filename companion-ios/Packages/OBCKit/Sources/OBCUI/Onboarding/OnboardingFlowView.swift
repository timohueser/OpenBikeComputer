import SwiftUI
import UniformTypeIdentifiers
import OBCDomain
import OBCTransport

/// Optional phone steps use the same firmware, import and sync operations as the Library.
public struct OnboardingFlowView: View {
    private let progress: OnboardingProgress
    private let firmware: FirmwareUpdateModel
    private let library: MainScreenModel
    private let onDemoRoute: () -> Void
    private let onImport: ([URL]) -> Void
    private let onUpdateAnswered: (String) -> Void
    private let onFinish: () -> Void
    @State private var showsFirmware = false
    @State private var showsImporter = false

    public init(
        progress: OnboardingProgress,
        firmware: FirmwareUpdateModel,
        library: MainScreenModel,
        onDemoRoute: @escaping () -> Void,
        onImport: @escaping ([URL]) -> Void,
        onUpdateAnswered: @escaping (String) -> Void = { _ in },
        onFinish: @escaping () -> Void
    ) {
        self.progress = progress
        self.firmware = firmware
        self.library = library
        self.onDemoRoute = onDemoRoute
        self.onImport = onImport
        self.onUpdateAnswered = onUpdateAnswered
        self.onFinish = onFinish
    }

    public var body: some View {
        Group {
            switch progress.stage {
            case .inactive, .sensors: sensors
            case .update: update
            case .route: route
            case .ride: ride
            case .done: Color.clear
            }
        }
        .task {
            progress.begin()
            library.start()
            firmware.start()
            skipCurrentFirmware()
        }
        .onChange(of: firmware.checkState) { _, _ in skipCurrentFirmware() }
        .onChange(of: firmware.runningVersion) { _, _ in skipCurrentFirmware() }
        .onChange(of: library.connectedScope) { _, _ in skipCurrentFirmware() }
        .onChange(of: firmware.phase) { _, phase in
            if phase == .done {
                showsFirmware = false
                if progress.stage == .update { progress.move(to: .route) }
            }
        }
        .sheet(isPresented: $showsFirmware, onDismiss: { firmware.start() }) {
            NavigationStack {
                FirmwareUpdateView(model: firmware)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Back to setup") { showsFirmware = false }
                                .disabled(firmware.phase == .transferring)
                        }
                    }
            }
            .interactiveDismissDisabled(firmware.phase == .transferring)
        }
        .fileImporter(
            isPresented: $showsImporter,
            allowedContentTypes: [UTType(filenameExtension: "gpx") ?? .xml,
                                  UTType(filenameExtension: "tcx") ?? .xml],
            allowsMultipleSelection: false
        ) { result in
            if case .success(let urls) = result { onImport(urls) }
        }
    }

    private var sensors: some View {
        LaunchScreenScaffold {
            VStack(spacing: 24) {
                DeviceGlyphView(variant: .home(name: library.deviceName))
                    .accessibilityHidden(true)
                LaunchTitle("Look at your OBC")
                    .accessibilityIdentifier("onboarding.sensorsTitle")
                LaunchMessage("Set up sensors and effort zones on the OBC. You can skip either step and add them later in Settings.")
                VStack(alignment: .leading, spacing: 18) {
                    tip("heart", "Heart rate", "Wear the strap to wake it.")
                    tip("bolt", "Power and cadence", "Turn the cranks to wake them.")
                }
                LaunchMessage("Sensors connect to the OBC itself. Continue here when you are ready.")
            }
        } actions: {
            Button("Continue") {
                progress.move(to: .update)
                skipCurrentFirmware()
            }
            .buttonStyle(.obcPrimary)
            .accessibilityIdentifier("onboarding.sensorsContinue")
        }
    }

    private var update: some View {
        LaunchScreenScaffold {
            VStack(spacing: 24) {
                Image(systemName: "arrow.down.circle")
                    .font(.system(.largeTitle)).foregroundStyle(OBCTheme.tint)
                    .accessibilityHidden(true)
                LaunchTitle(updateTitle)
                    .accessibilityIdentifier("onboarding.updateTitle")
                LaunchMessage(updateMessage)
                if firmware.checkState == .checking {
                    ProgressView("Checking for an update")
                } else if firmware.updateStatus == .available {
                    Text("\(firmware.runningVersionLine) → \(firmware.latestVersionLine)")
                        .font(.headline.monospacedDigit()).foregroundStyle(OBCTheme.ink)
                }
            }
        } actions: {
            if firmware.updateStatus == .available || library.protocolMismatch.map({ $0.found < $0.expected }) == true {
                Button("View update") {
                    answerVisibleUpdate()
                    showsFirmware = true
                }
                .buttonStyle(.obcPrimary)
                .accessibilityIdentifier("onboarding.updateNow")
            }
            Button(library.protocolMismatch == nil ? "Later" : "Finish setup") {
                answerVisibleUpdate()
                if library.protocolMismatch == nil { progress.move(to: .route) }
                else { finish() }
            }
            .buttonStyle(.obcGhost)
            .accessibilityIdentifier("onboarding.updateLater")
        }
    }

    private var updateTitle: String {
        if let mismatch = library.protocolMismatch {
            return mismatch.found > mismatch.expected ? "Update the app" : "An update is needed"
        }
        return firmware.checkState == .checking ? "Check your firmware" : "Update your OBC"
    }

    private var updateMessage: String {
        if let mismatch = library.protocolMismatch {
            return mismatch.found > mismatch.expected
                ? "This OBC needs a newer app for routes and ride sync. You can still ride and finish setup."
                : "Routes and ride sync need newer OBC firmware. Riding still works, and you can finish setup now."
        }
        return "Install the latest improvements, or do this later in Settings. Confirm the installation on your OBC."
    }

    private var route: some View {
        LaunchScreenScaffold {
            VStack(spacing: 24) {
                Image(systemName: "point.topleft.down.to.point.bottomright.curvepath")
                    .font(.system(.largeTitle)).foregroundStyle(OBCTheme.tint)
                    .accessibilityHidden(true)
                LaunchTitle("Send your first route")
                    .accessibilityIdentifier("onboarding.routeTitle")
                LaunchMessage("Try the Grimsel Pass demo route, or import a GPX or TCX file. Your OBC shows a confirmation when the route arrives.")
                if !canTransfer { connectionMessage }
            }
        } actions: {
            Button("Send demo route", action: onDemoRoute)
                .buttonStyle(.obcPrimary).disabled(!canTransfer)
                .accessibilityIdentifier("onboarding.demoRoute")
            Button("Import a route") { showsImporter = true }
                .buttonStyle(.obcGhost).disabled(!canTransfer)
                .accessibilityIdentifier("onboarding.importRoute")
            Button("Skip for now") { progress.move(to: .ride) }
                .buttonStyle(.obcGhost)
                .accessibilityIdentifier("onboarding.routeSkip")
        }
    }

    private var ride: some View {
        LaunchScreenScaffold {
            VStack(spacing: 24) {
                Image(systemName: "arrow.down.to.line")
                    .font(.system(.largeTitle)).foregroundStyle(OBCTheme.tint)
                    .accessibilityHidden(true)
                LaunchTitle(hasDemoRide ? "Your demo ride is here" : "Bring a ride home")
                    .accessibilityIdentifier("onboarding.rideTitle")
                LaunchMessage(hasDemoRide
                    ? "Find the Grimsel Pass ride in your Library. It is marked Demo and stays out of your totals."
                    : "Sync the demo ride from your OBC to see how rides reach your phone. It stays out of your totals.")
                if library.sync.syncState == .syncing {
                    ProgressView("Syncing rides…")
                } else if let interruption = library.sync.syncInterruption {
                    LaunchMessage(interruption.message)
                } else if library.sync.lastSyncCount != nil && !hasDemoRide {
                    LaunchMessage("No demo ride was found. You can sync your own rides from the Library after your first ride.")
                }
                if !canTransfer { connectionMessage }
            }
        } actions: {
            if !hasDemoRide {
                Button(library.sync.syncInterruption?.actionTitle ?? "Sync demo ride") {
                    if library.sync.syncInterruption != nil { library.sync.resumeSync() }
                    else { library.sync.sync() }
                }
                .buttonStyle(.obcPrimary)
                .disabled(!canTransfer || library.sync.syncState == .syncing)
                .accessibilityIdentifier("onboarding.syncRide")
            }
            Button(hasDemoRide ? "Open Library" : "Skip for now", action: finish)
                .buttonStyle(.obcGhost)
                .disabled(library.sync.syncState == .syncing)
                .accessibilityIdentifier("onboarding.finish")
        }
    }

    private var canTransfer: Bool {
        library.connection == .connected && library.connectedScope != nil && library.protocolMismatch == nil
    }

    private var hasDemoRide: Bool { library.rides.contains(where: \.isDemo) }

    private var connectionMessage: some View {
        LaunchMessage(library.protocolMismatch != nil
            ? "The app and OBC need compatible versions before routes or rides can be sent. You can skip this step."
            : "Keep your OBC on and nearby. You can skip this step and connect later.")
    }

    private func skipCurrentFirmware() {
        guard progress.stage == .update, library.connectedScope != nil,
              library.protocolMismatch == nil, firmware.hasUpdateAnswer,
              firmware.checkState != .checking, firmware.updateStatus != .available else { return }
        progress.move(to: .route)
    }

    private func finish() {
        firmware.stop()
        progress.finish()
        onFinish()
    }

    private func answerVisibleUpdate() {
        guard firmware.checkState != .checking, firmware.updateStatus == .available,
              let version = firmware.latestRelease?.version else { return }
        onUpdateAnswered(version)
    }

    private func tip(_ icon: String, _ title: String, _ message: String) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: icon).foregroundStyle(OBCTheme.tint).frame(width: 24)
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline).foregroundStyle(OBCTheme.ink)
                Text(message).font(.subheadline).foregroundStyle(OBCTheme.secondary)
            }
        }
        .accessibilityElement(children: .combine)
    }
}
