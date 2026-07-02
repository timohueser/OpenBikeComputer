import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The launch branch + first-run pairing flow (B2, design screens A · D1–D5 ·
/// H7 · H8). One `@Observable` state machine the host view renders:
///
/// ```
/// start() ─ bonded? ──yes──► connecting (A) ── connect ⊻ grace ──► main
///              │                                (never an error — out of range
///              no                                degrades to main + S4 banner)
///              ▼
///          pairIntro (D1) ─ startPairing ─► scanning (D2) ── found ─► row tap
///              ▲                              │    │                     │
///              └── help ── pairFailed (D5) ◄──┘    └► radioBlocked    pairing (D3)
///                              ▲   │ retry              (H7/H8)         │
///                              └───┘                                  paired (D4) ─► main
/// ```
///
/// Depends only on `DeviceTransport` + `BondStore` (the golden rule) — pairing
/// *is* `connect()`: on the real path iOS raises the system pairing sheet
/// mid-connect once the firmware requires encryption (A8); the mock resolves it
/// through `MockControl`'s radio/pairing gates. "Have we bonded before" comes
/// from the `BondStore`, never a CoreBluetooth detour.
@MainActor @Observable
public final class LaunchFlowModel {
    /// Why pairing failed — selects the D5 copy variant.
    public enum PairingFailure: Equatable, Sendable {
        /// Scan ended without finding the device (`DeviceError.deviceNotFound`).
        case timeout
        /// Found it, but pairing/connection didn't complete (declined sheet,
        /// link error).
        case rejected
    }

    /// Why the radio is unusable — H8 (off) vs the post-denial H7 state.
    public enum RadioBlock: Equatable, Sendable {
        case off
        case denied
    }

    /// The device row that slides into the D2 scanning screen.
    public struct DiscoveredDevice: Equatable, Sendable {
        /// The clean device name ("Trailhead") — what D4 greets with and what
        /// the bond record stores.
        public var name: String

        public init(name: String) {
            self.name = name
        }

        /// What the device advertises ("OBC-Trailhead"), per the design's row.
        public var advertisedName: String {
            name.hasPrefix("OBC-") ? name : "OBC-\(name)"
        }
    }

    /// The screen being shown. The host view switches over this exhaustively.
    public enum Phase: Equatable, Sendable {
        /// Pre-`start()` blank (parchment) — reads as the launch screen.
        case idle
        /// A — bonded, quietly reconnecting. Always resolves to `.main`.
        case connecting(deviceName: String)
        /// D1 — the friendly pairing prompt.
        case pairIntro
        /// D2 — scanning; the row slides in when `discovered` is non-nil.
        case scanning(discovered: DiscoveredDevice?)
        /// D3 — the beat while the (system) pairing completes.
        case pairing
        /// D4 — success.
        case paired(deviceName: String)
        /// D5 — calm timeout / failure with retry.
        case pairFailed(PairingFailure)
        /// H8 / H7-denied — the radio is off or Bluetooth access was denied.
        case radioBlocked(RadioBlock)
        /// Hand over to the main screen (B3; a placeholder until it lands).
        case main
    }

    /// Flow pacing — injectable so the model tests run in milliseconds.
    public struct Timing: Sendable {
        /// How long the A state may hold before landing on main regardless
        /// ("never a blocking full-screen spinner" — connect keeps trying in
        /// the background and the S4 banner covers the degraded case).
        public var connectGrace: Duration
        /// The D2 scan window; expiry is the D5 "we scanned for 30 seconds" copy.
        public var scanTimeout: Duration
        /// The D3 pause after the row tap (where the system sheet sits on the
        /// real path).
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

    private let transport: any DeviceTransport
    private let bondStore: any BondStore
    private let timing: Timing
    @ObservationIgnored private var flowTask: Task<Void, Never>?

    public init(
        transport: any DeviceTransport,
        bondStore: any BondStore,
        timing: Timing = Timing()
    ) {
        self.transport = transport
        self.bondStore = bondStore
        self.timing = timing
    }

    // MARK: The launch branch

    /// Check the bond and branch (call once, from the host's `.task`).
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
            // The state stream replays the latest value — already connected (or
            // degraded but known) means there is nothing to wait for.
            var current: ConnectionState?
            for await state in transport.state { current = state; break }
            if current != .connected && current != .outOfRange {
                // Race connect against the grace cap. Either way we land on
                // main: success connects silently, failure/expiry degrades to
                // the S4 disconnected banner — never an error screen.
                await withTaskGroup(of: Void.self) { group in
                    group.addTask { try? await transport.connect() }
                    group.addTask { try? await Task.sleep(for: timing.connectGrace) }
                    await group.next()
                    group.cancelAll()
                }
            }
            guard !Task.isCancelled else { return }
            phase = .main
        }
    }

    // MARK: The pairing flow

    /// D1 "Start pairing" (also D5 "Try again"): scan + connect under the
    /// 30-second window, then surface the found device as the D2 row.
    public func startPairing() {
        flowTask?.cancel()
        phase = .scanning(discovered: nil)
        flowTask = Task { [transport, timing] in
            do {
                try await Self.withScanWindow(timing.scanTimeout) {
                    try await transport.connect()
                }
                // Link up → the device exists; let the row slide in and wait
                // for the rider's tap.
                let name = (try? await transport.deviceInfo().name) ?? "OBC"
                guard !Task.isCancelled else { return }
                phase = .scanning(discovered: DiscoveredDevice(name: name))
            } catch {
                guard !Task.isCancelled else { return }
                phase = Self.failurePhase(for: error)
            }
        }
    }

    /// D2 row tap: hold the D3 beat (the system pairing sheet's slot on the
    /// real path), record the bond, celebrate.
    public func confirmPairing() {
        guard case .scanning(.some(let device)) = phase else { return }
        phase = .pairing
        flowTask = Task { [bondStore, timing] in
            try? await Task.sleep(for: timing.pairingBeat)
            guard !Task.isCancelled else { return }
            bondStore.save(BondRecord(deviceName: device.name))
            phase = .paired(deviceName: device.name)
        }
    }

    /// D4 "Go to routes".
    public func finishPairing() {
        phase = .main
    }

    /// D5 "Try again" — loops back to scanning.
    public func retryPairing() {
        startPairing()
    }

    /// D5 "Pairing help" — back to the D1 steps.
    public func showPairingHelp() {
        flowTask?.cancel()
        phase = .pairIntro
    }

    /// D2 "Cancel": stop the scan (or drop a half-open link) and step back.
    public func cancelScanning() {
        flowTask?.cancel()
        flowTask = Task { [transport] in
            await transport.disconnect()
        }
        phase = .pairIntro
    }

    /// H8/H7 secondary action — the library never locks (S-state law).
    public func browseLibrary() {
        flowTask?.cancel()
        phase = .main
    }

    /// H2 (Settings → Forget device): the bond record is already cleared and
    /// the link dropped by the Settings flow — cancel anything in flight and
    /// return to the D1 pairing prompt.
    public func forgetDevice() {
        flowTask?.cancel()
        phase = .pairIntro
    }

    // MARK: Helpers

    /// Run `connect` under the scan window; expiry throws `deviceNotFound`
    /// (the D5 timeout copy). Note the real transport's scan keeps running
    /// after a cancel today — stopping it early is A8 bring-up polish.
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
            // First child to finish decides; the loser's error (if any) is
            // discarded with the group.
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
        default:
            return .pairFailed(.rejected)
        }
    }
}
