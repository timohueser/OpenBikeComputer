import Foundation
import Observation
import OBCDomain
import OBCTransport

/// Launch, nearby-device selection and pairing. Optional setup owns its own progress after `.setup`.
@MainActor @Observable
public final class LaunchFlowModel {
    public enum PairingFailure: Equatable, Sendable {
        case timeout
        case rejected

        public var title: String {
            switch self {
            case .timeout: "Couldn't find your OBC"
            case .rejected: "Pairing didn't finish"
            }
        }

        public var reason: String {
            switch self {
            case .timeout:
                "We scanned for 30 seconds and didn't see it. Check that:"
            case .rejected:
                "If the code was wrong, try again. If the OBC is already paired to another phone, open Settings ▸ Connections on the OBC, hold Forget phone, then try again."
            }
        }
    }

    public enum RadioBlock: Equatable, Sendable { case off, denied }

    public enum Phase: Equatable, Sendable {
        case idle
        case welcome
        case pairIntro
        case bluetoothPermission
        case scanning(devices: [PairingDevice])
        case pairing
        case paired(deviceName: String)
        case pairFailed(PairingFailure)
        case radioBlocked(RadioBlock)
        case setup
        case main
    }

    public struct Timing: Sendable {
        public var scanTimeout: Duration
        public var pairingBeat: Duration

        public init(
            scanTimeout: Duration = .seconds(30),
            pairingBeat: Duration = .milliseconds(700)
        ) {
            self.scanTimeout = scanTimeout
            self.pairingBeat = pairingBeat
        }
    }

    public private(set) var phase: Phase = .idle
    public var deviceNameDraft = ""
    public private(set) var savingName = false
    public private(set) var nameSaveError: String?
    public var canSaveName: Bool { !savingName && !DeviceRenaming.normalized(deviceNameDraft).isEmpty }

    private let transport: any DeviceLink & DevicePairing & DeviceConfiguration
    private let bondStore: any BondStore
    private let timing: Timing
    private let onboardingPending: @MainActor () -> Bool
    @ObservationIgnored private var flowTask: Task<Void, Never>?
    @ObservationIgnored private var connectAttempt: Task<Void, Never>?
    @ObservationIgnored private var teardownTask: Task<Void, Never>?

    public init(
        transport: any DeviceLink & DevicePairing & DeviceConfiguration,
        bondStore: any BondStore, timing: Timing = Timing(),
        onboardingPending: @escaping @MainActor () -> Bool = { false }
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.timing = timing
        self.onboardingPending = onboardingPending
    }

    deinit {
        flowTask?.cancel()
        connectAttempt?.cancel()
        teardownTask?.cancel()
    }

    public func start() {
        guard phase == .idle else { return }
        guard bondStore.load() != nil else { phase = .welcome; return }
        phase = onboardingPending() ? .setup : .main
        // The Library observes link status. Reconnection never owns navigation.
        connectAttempt = Task { [transport] in
            for await state in transport.state {
                guard !Task.isCancelled, state == .disconnected else { return }
                try? await transport.connect()
                return
            }
        }
    }

    public static func acceptsPairingLink(_ url: URL) -> Bool {
        guard let link = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return false }
        return link.scheme?.lowercased() == "https" && link.host?.lowercased() == "openbikecomputer.com"
            && link.percentEncodedPath == "/app" && link.query == nil && link.fragment == nil
            && link.user == nil && link.password == nil && link.port == nil
    }

    /// The fixed QR link selects the pairing door, never a different device or an existing bond.
    public func openPairingLink() {
        switch phase {
        case .scanning, .pairing, .paired, .setup: return
        default: break
        }
        if bondStore.load() != nil {
            if phase == .idle { start() }
        } else {
            startPairing()
        }
    }

    public func showSwitchOn() {
        cancelPairingWork()
        phase = .pairIntro
    }

    public func showWelcome() {
        cancelPairingWork()
        phase = .welcome
    }

    /// Present the explanation before anything can instantiate the Bluetooth manager.
    public func startPairing() {
        guard bondStore.load() == nil else { return }
        cancelPairingWork()
        phase = .bluetoothPermission
    }

    public func allowBluetooth() {
        guard phase == .bluetoothPermission || isRetryScreen else { return }
        cancelPairingWork()
        let teardown = teardownTask
        phase = .scanning(devices: [])
        flowTask = Task { [transport, timing] in
            await teardown?.value
            guard !Task.isCancelled else { return }
            do {
                let devices = try await Self.withScanWindow(timing.scanTimeout) {
                    try await transport.scanForPairing()
                }
                guard !Task.isCancelled else { return }
                guard !devices.isEmpty else { throw DeviceError.deviceNotFound }
                if devices.count == 1 {
                    await pair(devices[0])
                } else {
                    phase = .scanning(devices: devices)
                }
            } catch {
                guard !Task.isCancelled else { return }
                await transport.disconnect()
                guard !Task.isCancelled else { return }
                phase = Self.failurePhase(for: error)
            }
        }
    }

    public func confirmPairing(_ device: PairingDevice) {
        guard case .scanning(let devices) = phase, devices.contains(device) else { return }
        phase = .pairing
        flowTask = Task { await pair(device) }
    }

    private func pair(_ device: PairingDevice) async {
        phase = .pairing
        do {
            try await Self.withScanWindow(timing.scanTimeout) { [transport] in
                try await transport.discover(device)
            }
            try Task.checkCancellation()
            let name = (try? await transport.deviceInfo())?.name ?? device.name
            try await Task.sleep(for: timing.pairingBeat)
            try await transport.authenticate()
            try Task.checkCancellation()
            // Store the bond and advance together: Cancel cannot land in a success animation
            // after pairing has already succeeded.
            bondStore.save(BondRecord(deviceName: name))
            deviceNameDraft = name
            nameSaveError = nil
            phase = .paired(deviceName: name)
        } catch {
            guard !Task.isCancelled else { return }
            await transport.disconnect()
            guard !Task.isCancelled else { return }
            phase = Self.failurePhase(for: error)
        }
    }

    /// The name is committed on the OBC before the phone records it or advances.
    public func saveNameAndContinue() {
        guard case .paired(let current) = phase, canSaveName else { return }
        let name = DeviceRenaming.normalized(deviceNameDraft)
        guard name != current else { finishPairing(); return }
        savingName = true
        nameSaveError = nil
        flowTask = Task { [transport, bondStore] in
            do {
                try await DeviceRenaming.save(name, to: transport)
                guard !Task.isCancelled else { return }
                bondStore.save(BondRecord(deviceName: name))
                deviceNameDraft = name
                savingName = false
                phase = .setup
            } catch {
                guard !Task.isCancelled else { return }
                savingName = false
                nameSaveError = "The name did not save. Keep your OBC nearby and try again, or keep its current name."
            }
        }
    }

    public func finishPairing() {
        guard case .paired = phase, !savingName else { return }
        phase = .setup
    }

    public func finishSetup() { phase = .main }

    public func replaySetup() {
        cancelPairingWork()
        phase = bondStore.load() == nil ? .welcome : .setup
    }

    public func retryPairing() { allowBluetooth() }
    public func showPairingHelp() { showSwitchOn() }
    public func cancelScanning() { showSwitchOn() }

    public func browseLibrary() {
        cancelPairingWork()
        phase = .main
    }

    public func forgetDevice() {
        cancelPairingWork()
        connectAttempt?.cancel()
        connectAttempt = nil
        phase = .welcome
    }

    private var isRetryScreen: Bool {
        switch phase {
        case .pairFailed, .radioBlocked: true
        default: false
        }
    }

    private func cancelPairingWork() {
        flowTask?.cancel()
        flowTask = nil
        switch phase {
        case .scanning, .pairing:
            let previous = teardownTask
            teardownTask = Task { [transport] in
                await previous?.value
                await transport.disconnect()
            }
        default: break
        }
    }

    private static func withScanWindow<T: Sendable>(
        _ window: Duration, _ operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(for: window)
                throw DeviceError.deviceNotFound
            }
            defer { group.cancelAll() }
            return try await group.next()!
        }
    }

    private static func failurePhase(for error: Error) -> Phase {
        switch error {
        case DeviceError.bluetoothUnavailable(.unauthorized): .radioBlocked(.denied)
        case DeviceError.bluetoothUnavailable: .radioBlocked(.off)
        case DeviceError.deviceNotFound: .pairFailed(.timeout)
        default: .pairFailed(.rejected)
        }
    }
}
