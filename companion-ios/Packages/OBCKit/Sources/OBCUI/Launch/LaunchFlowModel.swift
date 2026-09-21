import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The launch branch and the first-run pairing flow, as one `@Observable` state machine the
/// host view renders; `Phase` below lists the screens.
///
/// Depends only on `DeviceLink` and `BondStore`. Pairing is a two-phase connect: `startPairing`
/// runs the un-gated `discover()`, which surfaces the device row, and the row tap runs the gated
/// `authenticate()`, the op that raises the system passkey sheet. The sheet therefore lands in
/// the pairing beat, not on the scanning screen. "Have we bonded before" comes from the
/// `BondStore`, never from a CoreBluetooth detour.
@MainActor @Observable
public final class LaunchFlowModel {
    /// Why pairing failed; it selects the copy variant.
    public enum PairingFailure: Equatable, Sendable {
        /// Scan ended without finding the device (`DeviceError.deviceNotFound`).
        case timeout
        /// Found it, but pairing did not complete. Covers a declined or wrong passkey and a
        /// device that refuses because it is already bonded to another phone: the device
        /// suppresses its passkey and drops the link, and no distinguishable reason reaches the
        /// app, so this one case carries the combined copy.
        case rejected

        /// The headline for this failure. On the model, so the copy is testable without a view.
        public var title: String {
            switch self {
            case .timeout: "Couldn't find your OBC"
            case .rejected: "Pairing didn't finish"
            }
        }

        /// The body copy. For `.rejected` it is deliberately combined: a declined passkey and an
        /// already-bonded refusal arrive identically over the wire, so it offers both recoveries
        /// without claiming which one happened.
        public var reason: String {
            switch self {
            case .timeout:
                "We scanned for 30 seconds and didn't see it. A couple of things to check:"
            case .rejected:
                "Pairing didn't go through. If the passkey was wrong, try again. If the device is already paired to another phone, use Forget phone in its Bluetooth settings, then pair again."
            }
        }
    }

    /// Why the radio is unusable.
    public enum RadioBlock: Equatable, Sendable {
        case off
        case denied
    }

    /// The device row that slides into the scanning screen.
    public struct DiscoveredDevice: Equatable, Sendable {
        /// The clean device name, which the greeting shows and the bond record stores.
        public var name: String

        public init(name: String) {
            self.name = name
        }

        /// What the device advertises.
        public var advertisedName: String {
            name.hasPrefix("OBC-") ? name : "OBC-\(name)"
        }
    }

    /// The screen being shown. The host view switches over this exhaustively.
    public enum Phase: Equatable, Sendable {
        /// Pre-`start()` blank; it reads as the launch screen.
        case idle
        /// Bonded and quietly reconnecting. Resolves to `.main`, or to `.connectFailed` when the
        /// grace expires with the device silent.
        case connecting(deviceName: String)
        /// The bonded device did not answer within the grace window. The background attempt
        /// keeps listening either way.
        case connectFailed(deviceName: String)
        case pairIntro
        /// Scanning; the row slides in when `discovered` is non-nil.
        case scanning(discovered: DiscoveredDevice?)
        /// The beat while the system pairing completes.
        case pairing
        case paired(deviceName: String)
        case pairFailed(PairingFailure)
        /// The radio is off, or Bluetooth access was denied.
        case radioBlocked(RadioBlock)
        /// Hand over to the main screen.
        case main
    }

    /// Flow pacing, injectable so the model tests run in milliseconds.
    public struct Timing: Sendable {
        /// How long the connecting state may hold before it resolves: never a blocking
        /// full-screen spinner. The connect attempt keeps trying in the background either way.
        public var connectGrace: Duration
        /// The scan window; expiry drives the "we scanned for 30 seconds" copy.
        public var scanTimeout: Duration
        /// The minimum dwell after the gated `authenticate()` resolves, so the pairing beat is
        /// perceptible even when the mock authenticates instantly.
        public var pairingBeat: Duration

        public init(
            connectGrace: Duration = .seconds(8),
            scanTimeout: Duration = .seconds(30),
            pairingBeat: Duration = .milliseconds(700)
        ) {
            self.connectGrace = connectGrace
            self.scanTimeout = scanTimeout
            self.pairingBeat = pairingBeat
        }
    }

    public private(set) var phase: Phase = .idle

    private let transport: any DeviceLink
    private let bondStore: any BondStore
    private let timing: Timing
    @ObservationIgnored private var flowTask: Task<Void, Never>?
    /// The one background bonded-connect attempt (see `startConnectAttemptIfNeeded`).
    @ObservationIgnored private var connectAttempt: Task<Void, Never>?

    public init(
        transport: any DeviceLink,
        bondStore: any BondStore,
        timing: Timing = Timing()
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.timing = timing
    }

    deinit {
        flowTask?.cancel()
        connectAttempt?.cancel()
    }

    // MARK: The launch branch

    /// Check the bond and branch. Call once.
    public func start() {
        guard phase == .idle else { return }
        if let bond = bondStore.load() {
            beginBondedConnect(bond)
        } else {
            phase = .pairIntro
        }
    }

    private func beginBondedConnect(_ bond: BondRecord) {
        phase = .connecting(deviceName: bond.deviceName)
        flowTask = Task { [transport, timing] in
            // The state stream replays its latest value: already connected, or degraded but
            // known, means there is nothing to wait for, and out of range means the transport's
            // own reconnect loop is already on it.
            var current: ConnectionState?
            for await state in transport.state { current = state; break }
            if current == .connected || current == .outOfRange {
                phase = .main
                return
            }
            startConnectAttemptIfNeeded()
            // Watch for the link under the grace cap. The connect attempt runs unstructured,
            // because the real `connect()` is not cancellation-responsive while it scans for an
            // absent device: racing it inside a task group wedged the group's implicit drain.
            let connected = await Self.linkCameUp(transport.state, within: timing.connectGrace)
            guard !Task.isCancelled else { return }
            phase = connected ? .main : .connectFailed(deviceName: bond.deviceName)
        }
    }

    /// The background bonded-connect attempt, started at most once. While the device is out of
    /// reach the transport keeps scanning, so "Try again" re-watches this same attempt under a
    /// fresh grace window instead of stacking scans. When it resolves it finishes a still-waiting
    /// screen: a hard failure degrades to main, because the library never locks.
    private func startConnectAttemptIfNeeded() {
        guard connectAttempt == nil else { return }
        connectAttempt = Task { [transport, weak self] in
            try? await transport.connect()
            guard let self, !Task.isCancelled else { return }
            connectAttempt = nil
            switch phase {
            case .connecting, .connectFailed:
                flowTask?.cancel()
                phase = .main
            default:
                break
            }
        }
    }

    /// Whether `states` reports `.connected` within `grace`. Both children are
    /// cancellation-responsive, so the group's implicit drain cannot wedge.
    private static func linkCameUp(
        _ states: AsyncStream<ConnectionState>,
        within grace: Duration
    ) async -> Bool {
        await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                for await state in states where state == .connected { return true }
                return false
            }
            group.addTask {
                try? await Task.sleep(for: grace)
                return false
            }
            let first = await group.next() ?? false
            group.cancelAll()
            return first
        }
    }

    /// "Try again": back to connecting under a fresh grace window. A bond that vanished
    /// underneath, from a raced forget, falls back to the pairing prompt.
    public func retryConnect() {
        guard let bond = bondStore.load() else {
            phase = .pairIntro
            return
        }
        flowTask?.cancel()
        beginBondedConnect(bond)
    }

    // MARK: The pairing flow

    /// Scan and discover the un-gated surface under the scan window, then show the found device
    /// as a row. This is `discover()`, not `connect()`: touching a gated characteristic raises
    /// the passkey sheet, and that is deferred to the row tap.
    public func startPairing() {
        flowTask?.cancel()
        phase = .scanning(discovered: nil)
        flowTask = Task { [transport, timing] in
            do {
                try await Self.withScanWindow(timing.scanTimeout) {
                    try await transport.discover()
                }
                // The device exists; let the row slide in and wait for the rider's tap.
                let name = (try? await transport.deviceInfo().name) ?? "OBC"
                guard !Task.isCancelled else { return }
                phase = .scanning(discovered: DiscoveredDevice(name: name))
            } catch {
                guard !Task.isCancelled else { return }
                phase = Self.failurePhase(for: error)
            }
        }
    }

    /// The row tap: run the gated `authenticate()`, the operation that raises the system passkey
    /// sheet, inside the pairing beat that is already on screen. On success record the bond and
    /// celebrate; a decline drops to the failure screen.
    public func confirmPairing() {
        guard case .scanning(.some(let device)) = phase else { return }
        phase = .pairing
        flowTask = Task { [transport, bondStore, timing] in
            do {
                try await transport.authenticate()
            } catch {
                guard !Task.isCancelled else { return }
                phase = Self.failurePhase(for: error)
                return
            }
            // A minimum dwell, so success does not snap straight to the greeting.
            try? await Task.sleep(for: timing.pairingBeat)
            guard !Task.isCancelled else { return }
            bondStore.save(BondRecord(deviceName: device.name))
            phase = .paired(deviceName: device.name)
        }
    }

    public func finishPairing() {
        phase = .main
    }

    public func retryPairing() {
        startPairing()
    }

    public func showPairingHelp() {
        flowTask?.cancel()
        phase = .pairIntro
    }

    /// Stop the scan, or drop a half-open link, and step back.
    public func cancelScanning() {
        flowTask?.cancel()
        flowTask = Task { [transport] in
            await transport.disconnect()
        }
        phase = .pairIntro
    }

    /// The library never locks. A still-running connect attempt keeps listening, so the link
    /// comes up on its own once the device is nearby.
    public func browseLibrary() {
        flowTask?.cancel()
        phase = .main
    }

    /// The bond record is already cleared and the link dropped by the Settings flow; cancel
    /// anything in flight and return to the pairing prompt.
    public func forgetDevice() {
        flowTask?.cancel()
        connectAttempt?.cancel()  // its completion must not touch the phase now
        connectAttempt = nil
        phase = .pairIntro
    }

    // MARK: Helpers

    /// Run `connect` under the scan window; expiry throws `deviceNotFound`.
    private static func withScanWindow(
        _ window: Duration,
        _ connect: @escaping @Sendable () async throws -> Void
    ) async throws {
        try await withThrowingTaskGroup(of: Void.self) { group in
            group.addTask { try await connect() }
            group.addTask {
                try await Task.sleep(for: window)
                throw DeviceError.deviceNotFound
            }
            // First child to finish decides; the loser's error is discarded with the group.
            try await group.next()
            group.cancelAll()
        }
    }

    private static func failurePhase(for error: Error) -> Phase {
        switch error {
        case DeviceError.bluetoothUnavailable(.unauthorized):
            return .radioBlocked(.denied)
        case DeviceError.bluetoothUnavailable:
            return .radioBlocked(.off)
        case DeviceError.deviceNotFound:
            return .pairFailed(.timeout)
        case DeviceError.pairingFailed:
            // Declined or wrong passkey, or the encrypted link was refused.
            return .pairFailed(.rejected)
        default:
            return .pairFailed(.rejected)
        }
    }
}
