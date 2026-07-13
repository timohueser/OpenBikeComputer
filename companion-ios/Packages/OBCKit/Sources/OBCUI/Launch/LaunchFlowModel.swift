import Foundation
import Observation
import OBCDomain
import OBCTransport

/// The launch branch + first-run pairing flow (B2, design screens A · D1–D5 ·
/// H7 · H8). One `@Observable` state machine the host view renders:
///
/// ```
/// start() ─ bonded? ──yes──► connecting (A) ── link up ⊻ hard fail ──► main
///              │                  │ grace expired (device silent)
///              │                  ▼
///              │            connectFailed ── Try again ──► connecting (A)
///              │                  └─ Go to routes ──► main (S4 banner story)
///              no
///              ▼
///          pairIntro (D1) ─ startPairing ─► scanning (D2) ── found ─► row tap
///              ▲                              │    │                     │
///              └── help ── pairFailed (D5) ◄──┘    └► radioBlocked    pairing (D3)
///                              ▲   │ retry              (H7/H8)         │
///                              └───┘                                  paired (D4) ─► main
/// ```
///
/// Depends only on `DeviceTransport` + `BondStore` (the golden rule). Pairing is a
/// two-phase connect (#297): `startPairing` runs the un-gated `discover()` (surfaces
/// the D2 row), and the row tap runs the gated `authenticate()` — the op that raises
/// the system passkey sheet once the firmware requires encryption (A8), so the sheet
/// lands in the D3 beat, not on D2. The mock resolves both through `MockControl`'s
/// radio/pairing gates. "Have we bonded before" comes from the `BondStore`, never a
/// CoreBluetooth detour.
@MainActor @Observable
public final class LaunchFlowModel {
    /// Why pairing failed — selects the D5 copy variant.
    public enum PairingFailure: Equatable, Sendable {
        /// Scan ended without finding the device (`DeviceError.deviceNotFound`).
        case timeout
        /// Found it, but pairing/connection didn't complete. Covers both a
        /// declined / wrong passkey **and** the device refusing because it's
        /// already bonded to another phone (#455): the device suppresses its
        /// passkey and drops the link, and no distinguishable SMP reason reaches
        /// the app — CoreBluetooth reports only a generic pairing/connection
        /// failure (spec §8, `OBCProtocol.md`). So this one case carries the
        /// combined copy that names the already-paired possibility without
        /// asserting it (#461).
        case rejected

        /// D5 headline for this failure. Lives on the model (not the view) so the
        /// copy is testable without a rendered SwiftUI hierarchy.
        public var title: String {
            switch self {
            case .timeout: "Couldn't find your OBC"
            case .rejected: "Pairing didn't finish"
            }
        }

        /// D5 body copy. For `.rejected` this is deliberately *combined*: it can't
        /// tell a declined passkey from an already-bonded refusal (they arrive
        /// identically over the wire), so it offers both recoveries without
        /// claiming which one happened — retry the passkey, or, if the device is
        /// already paired to another phone, clear that bond first with Forget
        /// phone on the device.
        public var reason: String {
            switch self {
            case .timeout:
                "We scanned for 30 seconds and didn't see it. A couple of things to check:"
            case .rejected:
                "Pairing didn't go through. If the passkey was wrong, try again. If the device is already paired to another phone, use Forget phone in its Bluetooth settings, then pair again."
            }
        }
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
        /// A — bonded, quietly reconnecting. Resolves to `.main` (link up, or a
        /// hard connect failure that degrades) or `.connectFailed` (grace
        /// expired with the device silent — asleep / out of range).
        case connecting(deviceName: String)
        /// A-timeout — the bonded device didn't answer within the grace window.
        /// Try again re-enters A; Go to routes lands on main (the background
        /// attempt keeps listening either way).
        case connectFailed(deviceName: String)
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
        /// How long the A state may hold before it resolves ("never a blocking
        /// full-screen spinner"): the link coming up lands on main, expiry on
        /// the connect-failed screen — while the connect attempt keeps trying
        /// in the background.
        public var connectGrace: Duration
        /// The D2 scan window; expiry is the D5 "we scanned for 30 seconds" copy.
        public var scanTimeout: Duration
        /// The minimum D3 dwell after the gated `authenticate()` resolves, so the
        /// "pairing…" beat is perceptible even when the mock authenticates instantly
        /// (the real passkey sheet's own dwell already exceeds it).
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
    /// The one background bonded-connect attempt (see `startConnectAttemptIfNeeded`).
    @ObservationIgnored private var connectAttempt: Task<Void, Never>?

    public init(
        transport: any DeviceTransport,
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
            // degraded but known) means there is nothing to wait for, and out
            // of range means the transport's own reconnect loop is already on
            // it (no fresh attempt).
            var current: ConnectionState?
            for await state in transport.state { current = state; break }
            if current == .connected || current == .outOfRange {
                phase = .main
                return
            }
            startConnectAttemptIfNeeded()
            // Watch for the link under the grace cap. The connect attempt runs
            // unstructured (`connectAttempt`) because the real transport's
            // `connect()` is not cancellation-responsive while it scans for an
            // absent device: racing it *inside* a task group wedged the group's
            // implicit drain, holding A on screen forever.
            let connected = await Self.linkCameUp(transport.state, within: timing.connectGrace)
            guard !Task.isCancelled else { return }
            phase = connected ? .main : .connectFailed(deviceName: bond.deviceName)
        }
    }

    /// The background bonded-connect attempt — started at most once. While the
    /// device is out of reach the transport keeps scanning (its reconnect
    /// contract), so "Try again" re-watches this same attempt under a fresh
    /// grace window rather than stacking scans. When the attempt resolves it
    /// finishes a still-waiting A / connect-failed screen: success means the
    /// link is up; a hard failure (radio off/denied, connect refused) degrades
    /// to main — the library never locks, and the S4 banner owns the
    /// degraded-link story.
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
    /// cancellation-responsive (stream iteration + sleep), so the group's
    /// implicit drain cannot wedge.
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

    /// Connect-failed "Try again": back to A under a fresh grace window. The
    /// bond vanishing underneath (a raced forget) falls back to the D1 prompt.
    public func retryConnect() {
        guard let bond = bondStore.load() else {
            phase = .pairIntro
            return
        }
        flowTask?.cancel()
        beginBondedConnect(bond)
    }

    // MARK: The pairing flow

    /// D1 "Start pairing" (also D5 "Try again"): scan + discover the **un-gated**
    /// surface under the 30-second window, then surface the found device as the D2
    /// row. Crucially this is `discover()`, not `connect()` — touching a gated
    /// characteristic (which raises the LESC passkey sheet) is deferred to the row
    /// tap so the sheet lands in the D3 beat, not here on D2 (#297).
    public func startPairing() {
        flowTask?.cancel()
        phase = .scanning(discovered: nil)
        flowTask = Task { [transport, timing] in
            do {
                try await Self.withScanWindow(timing.scanTimeout) {
                    try await transport.discover()
                }
                // Link discovered (un-gated) → the device exists; let the row slide
                // in and wait for the rider's tap.
                let name = (try? await transport.deviceInfo().name) ?? "OBC"
                guard !Task.isCancelled else { return }
                phase = .scanning(discovered: DiscoveredDevice(name: name))
            } catch {
                guard !Task.isCancelled else { return }
                phase = Self.failurePhase(for: error)
            }
        }
    }

    /// D2 row tap: run the gated `authenticate()` — the operation that raises the
    /// system passkey sheet on the real path (A8), now landing inside the D3
    /// "pairing…" beat that's already on screen (#297). On success record the bond
    /// and celebrate; a decline drops to D5.
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
            // A minimum D3 dwell so success doesn't snap straight to D4 (the mock
            // authenticates instantly; the real passkey sheet's own dwell already
            // exceeds this).
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

    /// H8/H7 and connect-failed secondary action — the library never locks
    /// (S-state law). A still-running connect attempt keeps listening, so the
    /// link comes up on its own once the device is nearby.
    public func browseLibrary() {
        flowTask?.cancel()
        phase = .main
    }

    /// H2 (Settings → Forget device): the bond record is already cleared and
    /// the link dropped by the Settings flow — cancel anything in flight and
    /// return to the D1 pairing prompt.
    public func forgetDevice() {
        flowTask?.cancel()
        connectAttempt?.cancel()  // its completion must not touch the phase now
        connectAttempt = nil
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
        case DeviceError.pairingFailed:
            // Declined / wrong passkey, or the encrypted link was refused (A8).
            return .pairFailed(.rejected)
        default:
            return .pairFailed(.rejected)
        }
    }
}
