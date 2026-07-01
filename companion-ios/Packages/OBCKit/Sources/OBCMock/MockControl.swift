#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// Bluetooth radio power/permission, mapped onto the actionable `CBManagerState`
/// subset. Drives H7 (`.unauthorized`) / H8 (`.off`) via `connect()`.
public enum RadioState: Sendable, Equatable {
    case on
    case off
    case unauthorized
}

/// How a pairing attempt fails — drives the D5 variants. Kept mock-local (the wire
/// contract's `DeviceError` doesn't model pairing UX); `connect()` maps these onto
/// the closest existing `DeviceError` and the UI keys the exact copy off `scenario`.
public enum PairingFail: Sendable, Equatable {
    case timeout
    case rejected
}

/// A mid-session push the device (or radio) can originate. `emit(_:)` routes each
/// onto the live streams / fixture set so the UI updates without a re-fetch where a
/// stream exists (connection, battery); `rideAdded` mutates the enumerable set so
/// the next `listRides()` reflects it (there is no rides stream in `DeviceTransport`).
public enum DeviceEvent: Sendable {
    case connected
    case disconnected
    case outOfRange
    case batteryChanged(Int)
    case rideAdded(RideSummary)
}

/// The single, **live** fault-injection surface shared by the debug panel (B1P),
/// the tests, and `MockTransport`. A reference type on purpose: mutate it from
/// anywhere and every `MockTransport` reading it — and every open `state`/`battery`
/// stream — sees the change immediately.
///
/// `@unchecked Sendable` + an `NSLock` guarding the plain knobs (the two live
/// streams carry their own locks), matching `AsyncMulticast`'s pattern in
/// `OBCTransport`. All of this is `#if DEBUG` and never reaches a Release build.
public final class MockControl: @unchecked Sendable {
    // Live streams — thread-safe on their own; `connection`/`battery` are views onto them.
    let stateMulticast: AsyncMulticast<ConnectionState>
    let batteryMulticast: AsyncMulticast<Int>

    private let lock = NSLock()
    private var _scenario: Scenario
    private var _latency: Duration
    private var _throughput: Int
    private var _radio: RadioState
    private var _bonded: Bool
    private var _pairingFail: PairingFail?
    private var _pendingFailures: [DeviceError]
    private var _dropFraction: Double?
    private var _fixtures: FixtureSet

    /// Build from a named `Scenario`, loading its fixture set and applying its knobs.
    public init(scenario: Scenario = .happyPath) {
        let preset = scenario.preset
        let fixtures = FixtureSet.load(preset.fixtures)
        self.stateMulticast = AsyncMulticast(preset.connection)
        self.batteryMulticast = AsyncMulticast(fixtures.battery)
        self._scenario = scenario
        self._latency = preset.latency
        self._throughput = preset.throughputBytesPerSec
        self._radio = preset.radio
        self._bonded = preset.bonded
        self._pairingFail = preset.pairingFail
        self._pendingFailures = preset.pendingFailure.map { [$0] } ?? []
        self._dropFraction = preset.dropAtFraction
        self._fixtures = fixtures
    }

    /// Start from `happyPath` but override the reported device identity.
    public convenience init(deviceInfo: DeviceInfo) {
        self.init(scenario: .happyPath)
        self.deviceInfo = deviceInfo
    }

    // MARK: Live knobs (the issue's sketch — settable from the debug panel / tests)

    /// The active scenario. Setting it re-applies the whole preset (fixtures + knobs).
    public var scenario: Scenario {
        get { lock.withLocked { _scenario } }
        set { apply(newValue) }
    }

    /// Current connection state — a view onto the live `state` stream. Setting it
    /// pushes to every subscriber (force → S4 banner, `.connecting`, etc.).
    public var connection: ConnectionState {
        get { stateMulticast.value }
        set { stateMulticast.send(newValue) }
    }

    /// Battery percentage — a view onto the live `battery` stream. Nudge it and the
    /// top bar updates live.
    public var battery: Int {
        get { batteryMulticast.value }
        set { batteryMulticast.send(newValue) }
    }

    /// Per-op delay (the feel of a slow link).
    public var latency: Duration {
        get { lock.withLocked { _latency } }
        set { lock.withLocked { _latency = newValue } }
    }

    /// Realistic progress-bar speed (bytes/sec) for bulk transfers.
    public var throughputBytesPerSec: Int {
        get { lock.withLocked { _throughput } }
        set { lock.withLocked { _throughput = max(1, newValue) } }
    }

    /// Radio power/permission state — gates `connect()` (H7/H8).
    public var radio: RadioState {
        get { lock.withLocked { _radio } }
        set { lock.withLocked { _radio = newValue } }
    }

    /// Whether the app has bonded before — what `MockBondStore` serves to the
    /// B2 launch branch. Flip it live (panel) to replay first-run pairing.
    public var bonded: Bool {
        get { lock.withLocked { _bonded } }
        set { lock.withLocked { _bonded = newValue } }
    }

    /// The reported device identity (mirror of `fixtures.deviceInfo`).
    public var deviceInfo: DeviceInfo {
        get { lock.withLocked { _fixtures.deviceInfo } }
        set { lock.withLocked { _fixtures.deviceInfo = newValue } }
    }

    /// The current fixture set (routes / rides / config / diagnostics).
    public var fixtures: FixtureSet {
        get { lock.withLocked { _fixtures } }
        set { lock.withLocked { _fixtures = newValue } }
    }

    // MARK: Scenario / fixture control

    /// Apply a scenario: reload its fixtures and reset every knob + live stream.
    public func apply(_ scenario: Scenario) {
        let preset = scenario.preset
        let fixtures = FixtureSet.load(preset.fixtures)
        lock.withLocked {
            _scenario = scenario
            _latency = preset.latency
            _throughput = preset.throughputBytesPerSec
            _radio = preset.radio
            _bonded = preset.bonded
            _pairingFail = preset.pairingFail
            _pendingFailures = preset.pendingFailure.map { [$0] } ?? []
            _dropFraction = preset.dropAtFraction
            _fixtures = fixtures
        }
        stateMulticast.send(preset.connection)
        batteryMulticast.send(fixtures.battery)
    }

    /// Swap the fixture set by bundled-JSON name (`empty`, `large`, …) without
    /// touching the other knobs.
    public func loadFixtures(_ named: String) {
        let fixtures = FixtureSet.load(named)
        lock.withLocked { _fixtures = fixtures }
        batteryMulticast.send(fixtures.battery)
    }

    // MARK: Fault injection

    /// Fail the **next** throwing op (a control-plane read/write, or a transfer) with
    /// this error → S3 read error / upload fail. One-shot; a retry succeeds.
    public func failNextOp(_ error: DeviceError) {
        lock.withLocked { _pendingFailures.append(error) }
    }

    /// Arm the **next** transfer to drop at `fraction` of its bytes (0…1) → H10 sync
    /// interrupted / upload resume. The dropped transfer stalls with its stream open;
    /// `TransferHandle.resume()` restores the link and finishes it.
    public func dropTransfer(atFraction fraction: Double) {
        lock.withLocked { _dropFraction = min(max(0, fraction), 1) }
    }

    /// Force the radio state (H7/H8).
    public func setRadio(_ state: RadioState) { radio = state }

    /// Arm the next `connect()` to fail pairing (D5).
    public func failPairing(_ mode: PairingFail) {
        lock.withLocked { _pairingFail = mode }
    }

    /// Inject a mid-session device event. See `DeviceEvent`.
    public func emit(_ event: DeviceEvent) {
        switch event {
        case .connected: connection = .connected
        case .disconnected: connection = .disconnected
        case .outOfRange: connection = .outOfRange
        case .batteryChanged(let value): battery = value
        case .rideAdded(let ride):
            lock.withLocked { _fixtures.rides.insert(RideEntry(summary: ride), at: 0) }
        }
    }

    // MARK: Full-object access (mock helper, not part of DeviceTransport)

    /// The full `RouteBlob` (summary + waypoints + a synthesized payload) for a
    /// fixture route — what an on-device route detail / upload flow needs.
    public func routeBlob(for id: RouteID) -> RouteBlob? {
        lock.withLocked { _fixtures.routes.first { $0.summary.id == id } }?.blob()
    }

    // MARK: Transport-facing helpers (module-internal — MockTransport delegates here)

    /// Sleep the configured per-op latency (respects task cancellation).
    func delay() async {
        let duration = latency
        if duration > .zero { try? await Task.sleep(for: duration) }
    }

    /// The link is unusable only when down; `.outOfRange` still serves cached fixtures
    /// (S4 = content + banner, not an empty error screen).
    func requireReachable() throws {
        if connection == .disconnected { throw DeviceError.notConnected }
    }

    /// Throw one armed control-plane failure, if any (one-shot).
    func takePendingFailure() throws {
        let error: DeviceError? = lock.withLocked {
            _pendingFailures.isEmpty ? nil : _pendingFailures.removeFirst()
        }
        if let error { throw error }
    }

    /// Radio + pairing gate for `connect()`.
    func connectGate() throws {
        let (radio, pairing) = lock.withLocked { (_radio, _pairingFail) }
        switch radio {
        case .on: break
        case .off: throw DeviceError.bluetoothUnavailable(.poweredOff)
        case .unauthorized: throw DeviceError.bluetoothUnavailable(.unauthorized)
        }
        if let pairing {
            // Lossy map onto the wire error set; the UI picks the D5 copy from `scenario`.
            throw pairing == .timeout ? DeviceError.deviceNotFound : DeviceError.notConnected
        }
    }

    /// Update the on-device config; renaming (Delta 1) also updates the reported name.
    func setConfig(_ config: DeviceConfig) {
        lock.withLocked {
            _fixtures.config = config
            _fixtures.deviceInfo = _fixtures.deviceInfo.renamed(config.name)
        }
    }

    /// Remove a route from the fixture set (H11 swipe-to-delete persists).
    func removeRoute(_ id: RouteID) {
        lock.withLocked { _fixtures.routes.removeAll { $0.summary.id == id } }
    }

    /// Begin a throughput-paced simulated upload of `total` bytes. Returns an
    /// immediately-finished handle if the link is down (H4). Consumes an armed
    /// drop-fraction (or a pending failure → immediate drop).
    func beginTransfer(total: Int) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        return startTransfer(total: total, segments: [], rides: nil)
    }

    /// Begin a simulated ride download: one paced batch whose fixture rides land
    /// (with synthesized payloads) as their bytes complete. Link down (H4) or
    /// nothing to pull (H9 up to date) → both streams already finished.
    func beginRideDownload(_ ids: [RideID]) -> RideDownload {
        let wanted = Set(ids)
        let segments = lock.withLocked {
            _fixtures.rides.filter { wanted.contains($0.summary.id) }
        }.map { MockTransfer.Segment(id: $0.summary.id, byteCount: max(1, $0.downloadByteCount)) }
        let total = segments.reduce(0) { $0 + $1.byteCount }

        if connection == .disconnected { return .finished(.failed(.notConnected)) }   // H4
        if total == 0 { return .finished() }                                          // H9
        let (rideStream, rideContinuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        let handle = startTransfer(total: total, segments: segments, rides: rideContinuation)
        return RideDownload(handle: handle, rides: rideStream)
    }

    /// Shared pump setup: consume the one-shot fault knobs and start a `MockTransfer`.
    private func startTransfer(
        total: Int,
        segments: [MockTransfer.Segment],
        rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation?
    ) -> TransferHandle {
        let (dropFraction, throughput) = lock.withLocked { () -> (Double?, Int) in
            let armedFailure = _pendingFailures.isEmpty ? false : { _pendingFailures.removeFirst(); return true }()
            let drop = _dropFraction ?? (armedFailure ? 0.0 : nil)
            _dropFraction = nil
            return (drop, _throughput)
        }

        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let states = stateMulticast
        let transfer = MockTransfer(
            total: total, throughputBytesPerSec: throughput, dropAtFraction: dropFraction,
            segments: segments, rides: rides,
            linkChange: { state in states.send(state) }, progress: continuation,
            outcome: outcome
        )
        Task { await transfer.start() }
        return TransferHandle(
            progress: stream,
            outcome: outcome,
            onCancel: { Task { await transfer.cancel() } },
            onResume: { Task { await transfer.resume() } }
        )
    }
}

// MARK: - NSLock convenience (mirrors OBCTransport's lock/defer/unlock idiom)

extension NSLock {
    fileprivate func withLocked<T>(_ body: () -> T) -> T {
        lock(); defer { unlock() }; return body()
    }
}

extension DeviceInfo {
    /// A copy with a new name — the last-read `Config` name (Delta 1) surfacing in DIS.
    fileprivate func renamed(_ name: String) -> DeviceInfo {
        DeviceInfo(name: name, firmwareVersion: firmwareVersion, hardwareVersion: hardwareVersion,
                   serial: serial, protocolVersion: protocolVersion)
    }
}
#endif
