#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCDomain
import OBCProtocolV4

/// The **real** `DeviceTransport` (Tier 1 → CoreBluetooth). Scans for the OBC
/// service, connects, discovers DIS/BAS/OBC Control, reads the PSM and opens the
/// L2CAP CoC, and maps the semantic protocol onto GATT reads/writes/notifies +
/// the `BLEChannel` byte layer.
///
/// Ids on this transport's data plane are **device-namespace** (`DeviceObjectID`,
/// spec §4.1), enforced by the types (#359): route ops take the object id
/// directly, and ride ids are minted here via `RideID(deviceObjectID:)`. The
/// app's library ids never cross this boundary — the link between a library
/// route and its device copy is the persisted `deviceObjectID`.
///
/// Protocol operation state lives in the one ``TransferClient``. This type supplies only the
/// physical control-record inbox, CoC record channel, connection restoration, and BLE facts.
///
/// All mutable state is confined to a single serial `queue` (the CoreBluetooth
/// callback queue); async methods hop onto it and register continuations that the
/// delegate callbacks resolve. That confinement is why this can be a plain
/// `@unchecked Sendable` class rather than fighting `Sendable` on CoreBluetooth's
/// (non-`Sendable`) object graph.
public final class BLETransport: NSObject, DeviceTransport, @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.openbikecomputer.ble")
    private static let restorationIdentifier = "com.openbikecomputer.ble.central"
    private lazy var central = CBCentralManager(
        delegate: self,
        queue: queue,
        options: [CBCentralManagerOptionRestoreIdentifierKey: Self.restorationIdentifier]
    )
    private let discoveryStore: any BLEDiscoveryStore
    private var discoveryPolicy = BLEDiscoveryIntentPolicy()

    private let stateMulticast = AsyncMulticast<ConnectionState>(.disconnected)
    /// `nil` until the first real BAS value — the seed must not replay as "0%".
    private let batteryMulticast = AsyncMulticast<Int?>(nil)
    private let weatherRequestMulticast = AsyncMulticast<WeatherRequestEvent?>(nil)

    private var peripheral: CBPeripheral?
    private var activeScanServices: Set<BLEDiscoveryIntentPolicy.Service> = []
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    /// The live CoC byte pipe (`nil` until opened, or after a teardown). The
    /// `BLEChannel` wrapper is rebuilt around it on every (re)open.
    private var byteChannel: L2CAPByteChannel?
    private var bleChannel: BLEChannel?
    private lazy var transferClient = TransferClient(link: self)
    private var openingChannel = false
    private var channelWaiters: [CheckedContinuation<BLEChannel, Error>] = []

    // Watchdogs for the connect/CoC-open phases that can silently stall (#302):
    // an empty/partial GATT DB never fires `didDiscoverCharacteristicsFor`, and a
    // PSM read that never yields `didOpen` leaves `openingChannel` latched with
    // every future transfer parked. Each phase arms a one-shot on entry and
    // disarms it on the resolving callback; if it fires the phase is wedged and
    // gets unwound. Queue-confined like everything else.
    private var discoveryWatchdog: DispatchWorkItem?
    private var channelWatchdog: DispatchWorkItem?
    private static let phaseTimeout: DispatchTimeInterval = .seconds(10)
    /// The channel watchdog's budget across the gated PSM read while an
    /// `authenticate()` is parked: on a fresh pair iOS holds that read pending
    /// under the system passkey sheet while the rider reads the code off the
    /// device and types it — human-paced, so the machine-stall budget above
    /// would fail pairing at 10 s (and since the sheet stays up, pairing then
    /// completed *behind* the failure screen, which is why a retry succeeded
    /// instantly with no sheet). Once the read resolves, the watchdog re-arms
    /// at `phaseTimeout` for the machine-only openL2CAPChannel → didOpen tail.
    private static let pairingTimeout: DispatchTimeInterval = .seconds(90)
    // Outstanding operations (all touched only on `queue`). Connecting is a
    // two-phase flow (#297): `discover()` (un-gated) then `authenticate()` (gated,
    // raises the passkey sheet) — each parks its own continuation.
    private var discoverContinuation: CheckedContinuation<Void, Error>?
    private var authenticateContinuation: CheckedContinuation<Void, Error>?
    // One bounded read of the Weather Request context (spec §11). These fields are queue-confined
    // beside the foreground continuations above; there is no second manager/transport/session.
    private var weatherRequestWaiters: [UUID: CheckedContinuation<WeatherRequestRead, Error>] = [:]
    private var cancelledWeatherRequestWaiters: Set<UUID> = []
    private var weatherRequestDeadline: DispatchWorkItem?
    private var weatherRequestConnectedDeadline: DispatchWorkItem?
    /// The connected phase's **absolute** deadline, fixed when the connection came up. Held apart
    /// from the work item so re-arming a later stage cannot move it — see
    /// `armWeatherRequestConnectedDeadline()`.
    private var weatherRequestConnectedDeadlineAt: DispatchTime?
    private var weatherRequestReadInFlight = false
    private var weatherRequestStartedAt: ContinuousClock.Instant?
    private var weatherRequestDiscoveredAt: ContinuousClock.Instant?
    private var weatherRequestConnectedAt: ContinuousClock.Instant?
    private var weatherRequestReusedForeground = false
    private let weatherRequestClock = ContinuousClock()
    /// One request intent can keep a service-filtered background scan alive for at most 60 seconds.
    /// Once connected, GATT discovery + the 52-byte context read get at most 8 seconds of that
    /// budget. The deadline is *absolute*, not restarted per connection: a stray central that
    /// connects and drops repeatedly would otherwise extend a bounded window into a permanent
    /// background scan — a battery bug, and the same rule `obc_ble`'s advertising budget keeps.
    private static let weatherRequestBudget: TimeInterval = 60
    private static let weatherRequestConnectedBudget: DispatchTimeInterval = .seconds(8)
    // One bounded upload of a weather bundle (spec §11.5, WX9). Queue-confined like the read's
    // fields above; the two legs share the weather connection lane but never run concurrently —
    // the job engine sequences them with the network fetch in between, radio idle.
    private var weatherUploadWaiter: CheckedContinuation<WeatherBundleUpload, Error>?
    private var cancelledWeatherUploadTokens: Set<UUID> = []
    private var weatherUploadToken: UUID?
    private enum WeatherDelivery: Sendable {
        case bundle(Data)
        case unchanged(requestID: UInt32, retryAfterSeconds: UInt16)
    }
    private var weatherUploadPayload: WeatherDelivery?
    private var weatherUploadDeadline: DispatchWorkItem?
    private var weatherUploadConnectedDeadline: DispatchWorkItem?
    /// Absolute, like the read's — a budget is a deadline, not a restartable timer (§11.3).
    private var weatherUploadConnectedDeadlineAt: DispatchTime?
    private var weatherUploadInFlight = false
    private var weatherUploadStartedAt: ContinuousClock.Instant?
    private var weatherUploadConnectedAt: ContinuousClock.Instant?
    private var weatherUploadReusedForeground = false
    /// The running exchange, retained so ending the attempt can **cancel** it. Without this a
    /// superseded exchange keeps running against the live link and its verdict lands on whatever
    /// attempt happens to be registered when it arrives.
    private var weatherUploadExchange: Task<Void, Never>?
    /// When the current **weather-owned** connection came up. The connected budget belongs to the
    /// connection, not to a leg: a read that shared this link already spent part of it, so the
    /// upload's deadline is measured from here rather than from its own start (§11.3 — absolute
    /// deadlines, never restartable timers).
    private var weatherOwnedConnectionUpAt: DispatchTime?
    /// A restoration-adopted direct connect issued while the manager had not reached `.poweredOn`
    /// (`willRestoreState` runs *before* `centralManagerDidUpdateState`, and CoreBluetooth drops
    /// connects issued in `.unknown` on the floor). Re-issued on the first `.poweredOn`.
    private var weatherUploadRestoredConnectPending = false
    /// Overall upload-leg budget. Longer than the read's 60 s on purpose: this leg begins with a
    /// *pending direct connect* (no scan — after the served context read the device advertises OBC
    /// Control again, §11.3), which iOS holds until the peripheral is reachable.
    private static let weatherUploadBudget: TimeInterval = 90
    /// Connected budget for the upload leg: bonded re-encrypt + CoC open (~1–3 s) plus ≤ 64 KiB
    /// over the CoC (a couple of seconds) with margin for a slow link. Still comfortably inside a
    /// single background execution window.
    private static let weatherUploadConnectedBudget: DispatchTimeInterval = .seconds(25)
    /// True only across the #753 gated-phase retry beat — between a first,
    /// retryable gated failure (`resolveAuthenticateRetryable`) and the second
    /// attempt parking its continuation. In this window `authenticateContinuation`
    /// is momentarily `nil`, so a disconnect must be treated as terminal here
    /// (like a drop during a *pending* authenticate) rather than kicking the
    /// reconnect loop — otherwise it could re-raise the passkey behind D5.
    private var awaitingGatedRetry = false
    /// The beat before the one #753 gated-phase retry — long enough for the
    /// firmware's post-PairingComplete window (bond save under the GATT-serve
    /// lock) to drain, short enough to stay imperceptible inside the D3 beat.
    private static let gatedRetryBeat: Duration = .milliseconds(500)
    /// Services still awaiting their characteristics during `discover()`; discovery
    /// is done (the un-gated surface is ready) when it reaches zero.
    private var pendingServiceDiscovery = 0
    private var pendingReads: [CBUUID: [CheckedContinuation<Data, Error>]] = [:]
    private var pendingWrites: [CBUUID: [CheckedContinuation<Void, Error>]] = [:]

    // Physical indication records for protocol v4. Correlation, operation lifetime and STATUS
    // reconciliation belong to TransferClient; the transport only preserves received records.
    private var pendingObjectControlRecords: [Data] = []
    private var objectControlWaiters: [CheckedContinuation<Data, Error>] = []

    public override convenience init() {
        self.init(discoveryStore: UserDefaultsBLEDiscoveryStore())
    }

    init(discoveryStore: any BLEDiscoveryStore) {
        self.discoveryStore = discoveryStore
        super.init()
        // The standing weather watch survives relaunches: re-arm the policy from the persisted
        // flag *before* the manager exists, so a state-restoration launch has the scanning intent
        // in place when the delegate callbacks start arriving.
        if discoveryStore.weatherWatchArmed() {
            discoveryPolicy.setWeatherWatch(true)
        }
        _ = central  // force manager creation (and a state callback)
    }

    // MARK: DeviceTransport — lifecycle

    public var state: AsyncStream<ConnectionState> { stateMulticast.stream() }
    public var battery: AsyncStream<Int> {
        // Drop the not-yet-known seed: subscribers get the first *real* reading
        // (read at discovery + BAS notifies), never a fabricated 0%.
        let source = batteryMulticast.stream()
        return AsyncStream { continuation in
            let pump = Task {
                for await value in source {
                    if let value { continuation.yield(value) }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
        }
    }

    public var storeChanges: AsyncStream<StoreChanged> {
        AsyncStream { $0.finish() }
    }

    /// Replays the latest one-shot result, including a read that completed after CoreBluetooth
    /// restored the process — a caller that was not running when the request landed still sees it.
    /// This is transport evidence only; no scheduler/provider consumes it yet (WX3 is the seam,
    /// the bundle fetch is later epic work).
    public var weatherRequestEvents: AsyncStream<WeatherRequestEvent> {
        let source = weatherRequestMulticast.stream()
        return AsyncStream { continuation in
            let pump = Task {
                for await value in source {
                    if let value { continuation.yield(value) }
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in pump.cancel() }
        }
    }

    public func connect() async throws {
        // The full link is the two phases back to back. On a bonded reconnect this
        // raises no sheet (iOS re-encrypts from the stored keys); on a fresh pair it
        // would — which is why the launch flow calls the phases separately (#297).
        try await discover()
        try await authenticate()
    }

    public func discover() async throws {
        // Phase 1 (#297): scan → connect → discover services + the un-gated
        // characteristics only. Resolves once every service's characteristics are
        // in hand (so `deviceInfo()` can read DIS + `protocolVersion`), without ever
        // touching a gated characteristic.
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                discoverContinuation = cont
                _ = discoveryPolicy.requestForeground()
                startConnectIfReady()
            }
        }
    }

    public func authenticate() async throws {
        // Phase 2 (#297): the gated ops (subscribe the `status` notify, read the
        // PSM, open the CoC) that establish the encrypted, LESC-authenticated link
        // and raise the system passkey sheet. Resolves when the CoC opens.
        //
        // #753: on a fresh pair the gated phase can fail *once* in the firmware's
        // post-PairingComplete window even though SMP pairing completed and both
        // sides bonded — see `GatedPairingWindowError`. Retry the gated phase once,
        // after a short beat, on the now-bonded link (no passkey re-raise) rather
        // than dropping straight to D5. Only an auth-class-while-connected failure
        // is retried; a decline / link drop / CoC failure is terminal, as today.
        do {
            try await GatedPhaseRetry.runOnce(
                beat: Self.gatedRetryBeat,
                isRetryable: { $0 is GatedPairingWindowError },
                attempt: { [self] in try await runGatedPhaseOnce() }
            )
        } catch is GatedPairingWindowError {
            // The single retry also hit the pairing window — final. The retryable
            // resolve deliberately left the link + intent up for the retry, so tear
            // them down now (like `failAuthenticate`) and surface the D5 error.
            await teardownAfterFailedRetry()
            throw DeviceError.pairingFailed
        }
        // A terminal `DeviceError` from `runGatedPhaseOnce` already tore the intent
        // down (`failAuthenticate`) and isn't retryable, so `runOnce` rethrew it
        // straight through to the caller — no beat, no retry, D5 as today.
    }

    /// One gated-phase attempt (#753): park the authenticate continuation and kick
    /// `beginAuthenticate()`; resolves on CoC open, throws `GatedPairingWindowError`
    /// on a retryable failure or a plain `DeviceError` on a terminal one.
    private func runGatedPhaseOnce() async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                // This attempt is now live; from here `authenticateContinuation`
                // (not `awaitingGatedRetry`) owns drop handling.
                awaitingGatedRetry = false
                authenticateContinuation = cont
                beginAuthenticate()
            }
        }
    }

    public func disconnect() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                let cancelForegroundConnection = discoveryPolicy.cancelForeground()
                if cancelForegroundConnection, let peripheral { central.cancelPeripheralConnection(peripheral) }
                if central.isScanning, discoveryPolicy.scanServices.isEmpty { central.stopScan() }
                stateMulticast.send(.disconnected)
                // Any remaining weather intent (a pending one-shot or the standing watch) keeps
                // the radio; `startConnectIfReady` no-ops when nothing wants it.
                if discoveryPolicy.hasIntent { startConnectIfReady() }
                cont.resume()
            }
        }
    }

    // `suspendLink()` (#459) uses the protocol default — `disconnect()` — which
    // is already the full suspend: it drops `wantsConnect` (the latch every
    // reconnect re-issue in `didFailToConnect` / `didDisconnectPeripheral` and
    // every `startConnectIfReady` checks), cancels the pending connect iOS
    // holds, and stops the scan. That latch IS the background-reconnect loop's
    // pause switch: while it's down, nothing in the delegate flow re-raises
    // the link.

    public func resumeLink() async {
        // Foreground return (#459): re-arm the intent latch and let the existing
        // delegate flow re-raise the link — scan → didDiscover → connect →
        // discovery → `beginAuthenticate()` → CoC → `.connected`, the same
        // unsolicited bonded silent-reconnect path a mid-ride drop takes (no
        // passkey sheet; iOS re-encrypts from the stored keys). Deliberately
        // NOT `connect()`: that parks fresh discover/authenticate continuations,
        // clobbering (and leaking) any still waiting from a launch attempt the
        // suspend interrupted.
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                _ = discoveryPolicy.requestForeground()
                startConnectIfReady()
                cont.resume()
            }
        }
    }

    // MARK: One-shot Weather Request read (spec §11)

    /// Run one bounded, authenticated read of the `weatherRequestContext` characteristic against the
    /// known bonded peripheral, then let go of the link. Concurrent callers coalesce onto the same
    /// intent. The operation has no retry loop — a drop or read failure ends it, and
    /// timeout/cancellation stop its scan and disconnect **only** a connection it created, never a
    /// foreground session it happened to ride.
    public func readWeatherRequestContext() async throws -> WeatherRequestRead {
        let waiterID = UUID()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation {
                (continuation: CheckedContinuation<WeatherRequestRead, Error>) in
                queue.async { [self] in
                    if cancelledWeatherRequestWaiters.remove(waiterID) != nil {
                        continuation.resume(throwing: WeatherRequestError.cancelled)
                        return
                    }
                    registerWeatherRequestWaiter(waiterID, continuation)
                }
            }
        } onCancel: { [weak self] in
            self?.queue.async { [weak self] in self?.cancelWeatherRequestWaiter(waiterID) }
        }
    }

    private func registerWeatherRequestWaiter(
        _ id: UUID, _ continuation: CheckedContinuation<WeatherRequestRead, Error>
    ) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard let knownID = discoveryStore.knownPeripheralID() else {
            continuation.resume(throwing: WeatherRequestError.noKnownBondedPeripheral)
            return
        }
        weatherRequestWaiters[id] = continuation
        if discoveryPolicy.weatherRequestPending { return } // coalesced caller

        let now = weatherRequestClock.now
        weatherRequestStartedAt = now
        weatherRequestDiscoveredAt = nil
        weatherRequestConnectedAt = nil
        weatherRequestConnectedDeadlineAt = nil
        weatherRequestReusedForeground = false
        let deadline = Date().addingTimeInterval(Self.weatherRequestBudget)
        discoveryStore.armWeatherRestoration(until: deadline)
        armWeatherRequestDeadline(until: deadline)

        let connectedID = peripheral?.state == .connected ? peripheral?.identifier : nil
        switch discoveryPolicy.requestWeather(knownPeripheralID: knownID, connectedPeripheralID: connectedID) {
        case .readOnExistingConnection:
            weatherRequestDiscoveredAt = now
            weatherRequestConnectedAt = now
            weatherRequestReusedForeground = true
            beginWeatherRequestReadIfReady()
        case .scan:
            startConnectIfReady()
        case .waitForCurrentConnection:
            break
        }
    }

    private func cancelWeatherRequestWaiter(_ id: UUID) {
        dispatchPrecondition(condition: .onQueue(queue))
        if let continuation = weatherRequestWaiters.removeValue(forKey: id) {
            continuation.resume(throwing: WeatherRequestError.cancelled)
            if weatherRequestWaiters.isEmpty { failWeatherRequest(.cancelled, publish: false) }
        } else {
            // The cancellation handler may beat the registration hop onto this queue.
            cancelledWeatherRequestWaiters.insert(id)
        }
    }

    private func armWeatherRequestDeadline(until deadline: Date) {
        weatherRequestDeadline?.cancel()
        let remaining = deadline.timeIntervalSinceNow
        guard remaining > 0 else {
            failWeatherRequest(.timedOut)
            return
        }
        let item = DispatchWorkItem { [weak self] in self?.failWeatherRequest(.timedOut) }
        weatherRequestDeadline = item
        queue.asyncAfter(deadline: .now() + remaining, execute: item)
    }

    /// Bound the connected phase — **one deadline from the moment the connection came up**, never
    /// restarted by a later stage.
    ///
    /// This is called twice per connection (once when the link is up, once when the read is about to
    /// go out), and an earlier draft re-armed a fresh 8 s on each, so a slow service discovery
    /// followed by a slow read could spend the budget twice and hold the radio ~16 s. That is the
    /// same failure the device guards against on its side of the link — a budget must be a deadline,
    /// not a restartable timer (spec §11.3) — and the epic's ≤ 5 s median / ≤ 10 s p95 connected-time
    /// target is not a target anything can meet if the bound quietly doubles.
    private func armWeatherRequestConnectedDeadline() {
        dispatchPrecondition(condition: .onQueue(queue))
        weatherRequestConnectedDeadline?.cancel()
        let deadline = weatherRequestConnectedDeadlineAt ?? .now() + Self.weatherRequestConnectedBudget
        weatherRequestConnectedDeadlineAt = deadline
        let item = DispatchWorkItem { [weak self] in self?.failWeatherRequest(.timedOut) }
        weatherRequestConnectedDeadline = item
        queue.asyncAfter(deadline: deadline, execute: item)
    }

    private func beginWeatherRequestReadIfReady() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard discoveryPolicy.weatherRequestPending, !weatherRequestReadInFlight else { return }
        guard let knownID = discoveryStore.knownPeripheralID(), peripheral?.identifier == knownID else { return }
        guard characteristics[GATT.weatherRequestContext] != nil else {
            failWeatherRequest(.readFailed)
            return
        }
        weatherRequestReadInFlight = true
        if weatherRequestConnectedAt == nil { weatherRequestConnectedAt = weatherRequestClock.now }
        armWeatherRequestConnectedDeadline()
        Task { [weak self] in
            guard let self else { return }
            do {
                let data = try await self.read(GATT.weatherRequestContext)
                let context = try WeatherRequestContext(decoding: data)
                self.queue.async { [weak self] in self?.completeWeatherRequest(context) }
            } catch let error as WeatherRequestError {
                self.queue.async { [weak self] in self?.failWeatherRequest(error) }
            } catch {
                self.queue.async { [weak self] in self?.failWeatherRequest(.readFailed) }
            }
        }
    }

    private func completeWeatherRequest(_ context: WeatherRequestContext) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard discoveryPolicy.weatherRequestPending else { return }
        let now = weatherRequestClock.now
        let start = weatherRequestStartedAt ?? now
        let discovered = weatherRequestDiscoveredAt ?? now
        let connected = weatherRequestConnectedAt ?? discovered
        let result = WeatherRequestRead(
            context: context,
            discoveryLatency: start.duration(to: discovered),
            connectedDuration: connected.duration(to: now),
            reusedForegroundConnection: weatherRequestReusedForeground
        )
        if let id = peripheral?.identifier { discoveryStore.saveKnownPeripheralID(id) }
        let disconnectOwnedConnection = endWeatherRequestState()
        let waiters = weatherRequestWaiters.values
        weatherRequestWaiters.removeAll()
        for waiter in waiters { waiter.resume(returning: result) }
        weatherRequestMulticast.send(.completed(result))
        print(
            "[OBC BLE weather] request \(context.requestID) reason=\(context.reason.rawValue) "
                + "discovery=\(result.discoveryLatency) connected=\(result.connectedDuration) "
                + "reused=\(result.reusedForegroundConnection)"
        )
        if disconnectOwnedConnection {
            releaseWeatherOwnedConnection()
        } else if discoveryPolicy.weatherUploadPending {
            // An upload one-shot queued behind this read shares the kept connection (WX9). The
            // read-owned connection discovered only the weather service, so the control plane may
            // still be missing — fetch it, and the characteristic-discovery completion kicks the
            // upload.
            continueWeatherUploadOnSharedConnection()
        }
    }

    /// Release a weather-owned connection after its one-shot ends. Cancelling a **pending**
    /// connect delivers no delegate callback (only a connected peripheral produces
    /// `didDisconnectPeripheral`), so for anything not fully connected the policy phase must be
    /// unwound here — otherwise the lane would sit `.connecting` forever and park every later
    /// foreground connect behind it.
    private func releaseWeatherOwnedConnection() {
        dispatchPrecondition(condition: .onQueue(queue))
        if let peripheral, peripheral.state == .connected {
            central.cancelPeripheralConnection(peripheral)  // didDisconnectPeripheral cleans up
            return
        }
        if let peripheral { central.cancelPeripheralConnection(peripheral) }
        discoveryPolicy.didDisconnect()
        startConnectIfReady()
    }

    /// The read leg finished on a connection an upload is also waiting for: start the upload if
    /// the control plane is discovered, or discover it first (the completion re-enters
    /// `beginWeatherUploadIfReady`).
    private func continueWeatherUploadOnSharedConnection() {
        dispatchPrecondition(condition: .onQueue(queue))
        if characteristics[GATT.objectControl] != nil, characteristics[GATT.psm] != nil {
            beginWeatherUploadIfReady()
        } else if let peripheral, peripheral.state == .connected {
            peripheral.discoverServices([GATT.obcControlService, GATT.weatherRequestService])
        }
    }

    private func failWeatherRequest(_ error: WeatherRequestError, publish: Bool = true) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard discoveryPolicy.weatherRequestPending else { return }
        let disconnectOwnedConnection = endWeatherRequestState()
        let waiters = weatherRequestWaiters.values
        weatherRequestWaiters.removeAll()
        for waiter in waiters { waiter.resume(throwing: error) }
        if publish { weatherRequestMulticast.send(.failed(error)) }
        print("[OBC BLE weather] failed: \(error)")
        if disconnectOwnedConnection {
            releaseWeatherOwnedConnection()
        } else if discoveryPolicy.weatherUploadPending, discoveryPolicy.connectionOwnership == .weatherRequest {
            continueWeatherUploadOnSharedConnection()
        } else if central.isScanning, discoveryPolicy.scanServices.isEmpty {
            central.stopScan()
        }
    }

    private func endWeatherRequestState() -> Bool {
        let disconnectOwnedConnection = discoveryPolicy.finishWeatherRequest()
        weatherRequestDeadline?.cancel()
        weatherRequestDeadline = nil
        weatherRequestConnectedDeadline?.cancel()
        weatherRequestConnectedDeadline = nil
        weatherRequestConnectedDeadlineAt = nil
        weatherRequestReadInFlight = false
        discoveryStore.clearWeatherRestoration()
        return disconnectOwnedConnection
    }

    /// Arm the autonomous read the standing watch triggers (WX9): the same bookkeeping
    /// `registerWeatherRequestWaiter` does, minus a waiter — the result reaches its consumer via
    /// `weatherRequestEvents`, exactly like a read completed after state restoration. The policy
    /// has already raised `weatherRequestPending` (see `DiscoveryAction.connectForWeatherRead`).
    private func armAutonomousWeatherRead() {
        dispatchPrecondition(condition: .onQueue(queue))
        let now = weatherRequestClock.now
        weatherRequestStartedAt = now
        weatherRequestDiscoveredAt = nil
        weatherRequestConnectedAt = nil
        weatherRequestConnectedDeadlineAt = nil
        weatherRequestReusedForeground = false
        let deadline = Date().addingTimeInterval(Self.weatherRequestBudget)
        discoveryStore.armWeatherRestoration(until: deadline)
        armWeatherRequestDeadline(until: deadline)
    }

    /// Convert the live-link notification into the ordinary authenticated read transaction. The
    /// notification payload is deliberately not accepted as the request: firmware keeps the hint
    /// pending until this read response is served, giving the exchange an acknowledgement instead
    /// of losing a request after merely enqueueing an ATT notification.
    private func beginNotifiedWeatherRequestRead() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard !discoveryPolicy.weatherRequestPending,
              let knownID = discoveryStore.knownPeripheralID(),
              peripheral?.identifier == knownID,
              peripheral?.state == .connected
        else { return }

        let now = weatherRequestClock.now
        guard discoveryPolicy.requestWeather(
            knownPeripheralID: knownID,
            connectedPeripheralID: knownID
        ) == .readOnExistingConnection else { return }
        armAutonomousWeatherRead()
        weatherRequestDiscoveredAt = now
        weatherRequestConnectedAt = now
        weatherRequestReusedForeground = true
        beginWeatherRequestReadIfReady()
    }

    // MARK: One-shot Weather Bundle upload (spec §11.5, WX9)

    /// Arm or disarm the **standing weather watch**: scan for the Weather Request UUID whenever
    /// nothing else needs the radio, so a device raising a request wakes the app — in the
    /// foreground, in the background, and (via CoreBluetooth state restoration) after the process
    /// has been killed. The flag persists across relaunches. Ignored without a known authenticated
    /// peripheral (`startConnectIfReady` guards): with nothing bonded there is nothing to wake for.
    public func setWeatherWatch(_ enabled: Bool) {
        queue.async { [self] in
            discoveryStore.setWeatherWatchArmed(enabled)
            discoveryPolicy.setWeatherWatch(enabled)
            if enabled {
                startConnectIfReady()
            } else if central.isScanning, discoveryPolicy.scanServices.isEmpty {
                central.stopScan()
                activeScanServices.removeAll()
            }
        }
    }

    /// Upload one OBCW bundle as object type `20`, singleton id `0`, over the ordinary reliable
    /// CoC — the second connection of the §11 exchange. Rides an existing foreground session when
    /// one is up (and never tears it down); otherwise makes its own bounded ephemeral connection
    /// to the known bonded peripheral and disconnects when the verdict lands. Success is the
    /// device's `committed` — which per §11.6 includes the duplicate/stale ignored-but-successful
    /// rows, so retrying this call with the same bytes after an ambiguous failure is always safe.
    public func uploadWeatherBundle(_ payload: Data) async throws -> WeatherBundleUpload {
        guard !payload.isEmpty else { throw WeatherUploadError.emptyPayload }
        let token = UUID()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation {
                (continuation: CheckedContinuation<WeatherBundleUpload, Error>) in
                queue.async { [self] in
                    if cancelledWeatherUploadTokens.remove(token) != nil {
                        continuation.resume(throwing: WeatherUploadError.cancelled)
                        return
                    }
                    registerWeatherUpload(token, .bundle(payload), continuation)
                }
            }
        } onCancel: { [weak self] in
            self?.queue.async { [weak self] in self?.cancelWeatherUpload(token) }
        }
    }

    /// Answer a live weather request without opening a CoC or retransmitting the held bundle.
    /// Firmware that predates command 7 answers `unknownCommand`; surface that as `rejected` so
    /// the weather job can safely fall back to the ordinary full-bundle upload.
    public func acknowledgeWeatherUnchanged(
        requestID: UInt32, retryAfterSeconds: UInt16
    ) async throws -> WeatherBundleUpload {
        let token = UUID()
        let delivery = WeatherDelivery.unchanged(
            requestID: requestID, retryAfterSeconds: retryAfterSeconds
        )
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation {
                (continuation: CheckedContinuation<WeatherBundleUpload, Error>) in
                queue.async { [self] in
                    if cancelledWeatherUploadTokens.remove(token) != nil {
                        continuation.resume(throwing: WeatherUploadError.cancelled)
                        return
                    }
                    registerWeatherUpload(token, delivery, continuation)
                }
            }
        } onCancel: { [weak self] in
            self?.queue.async { [weak self] in self?.cancelWeatherUpload(token) }
        }
    }

    private func registerWeatherUpload(
        _ token: UUID, _ payload: WeatherDelivery,
        _ continuation: CheckedContinuation<WeatherBundleUpload, Error>
    ) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard weatherUploadWaiter == nil else {
            continuation.resume(throwing: WeatherUploadError.busy)
            return
        }
        guard discoveryStore.knownPeripheralID() != nil else {
            continuation.resume(throwing: WeatherUploadError.noKnownBondedPeripheral)
            return
        }
        // A restoration-adopted upload intent may already hold a connecting/connected weather
        // connection with no payload — this call supplies the bytes and **attaches to it**. Its
        // budgets are already running: re-arming them here would hand a single radio hold two
        // 25 s connected windows (adoption + registration) and push the persisted 90 s overall
        // deadline out by another 90. A budget survives a handoff by being carried, never reset.
        let adopted = discoveryPolicy.weatherUploadPending
        weatherUploadWaiter = continuation
        weatherUploadToken = token
        weatherUploadPayload = payload
        weatherUploadInFlight = false
        if !adopted {
            weatherUploadReusedForeground = false
            weatherUploadStartedAt = weatherRequestClock.now
            weatherUploadConnectedAt = nil
            weatherUploadConnectedDeadlineAt = nil
        }
        let fresh = Date().addingTimeInterval(Self.weatherUploadBudget)
        // Only an adopted (restoration) intent inherits the persisted deadline: after a
        // force-quit iOS discards restoration state, willRestoreState never clears the stored
        // key, and inheriting it here would time the relaunch resume out instantly (review
        // NEW-1) - the exact resume the checkpoint exists to serve.
        let deadline = adopted ? min(fresh, discoveryStore.weatherUploadRestorationDeadline() ?? fresh) : fresh
        discoveryStore.armWeatherUploadRestoration(until: deadline)
        armWeatherUploadDeadline(until: deadline)
        startWeatherUploadConnectIfReady()
    }

    /// Take the upload intent as far as the radio currently allows. Re-entered from
    /// `centralManagerDidUpdateState` because the engine's resume path can call
    /// `uploadWeatherBundle` at app launch, **before** the manager has reached `.poweredOn` —
    /// without the re-entry the attempt would sit parked until its overall deadline.
    ///
    /// A **restoration-adopted** intent has no waiter yet (the payload arrives later), so the
    /// pending-intent flag is part of the gate: without it the adopted leg had no `.poweredOn`
    /// re-entry at all and could never surface a hard-off radio.
    private func startWeatherUploadConnectIfReady() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard weatherUploadWaiter != nil || discoveryPolicy.weatherUploadPending,
              let knownID = discoveryStore.knownPeripheralID() else { return }
        switch central.state {
        case .poweredOn:
            break
        case .poweredOff, .unauthorized, .unsupported:
            failWeatherUpload(.bluetoothUnavailable)
            return
        default:
            return  // .unknown / .resetting — centralManagerDidUpdateState re-enters here
        }
        let connectedID = peripheral?.state == .connected ? peripheral?.identifier : nil
        switch discoveryPolicy.requestWeatherUpload(
            knownPeripheralID: knownID, connectedPeripheralID: connectedID
        ) {
        case .uploadOnExistingConnection:
            if weatherUploadConnectedAt == nil { weatherUploadConnectedAt = weatherRequestClock.now }
            weatherUploadReusedForeground = discoveryPolicy.connectionOwnership == .foreground
            beginWeatherUploadIfReady()
        case .connectDirect:
            guard let retrieved = central.retrievePeripherals(withIdentifiers: [knownID]).first else {
                failWeatherUpload(.noKnownBondedPeripheral)
                return
            }
            // Same rule as `didDiscover`: a connect claims the radio, so the standing watch's scan
            // stops here rather than running alongside it (scan + connect is the battery bug the
            // watch's whole gating exists to avoid). `didDisconnect` re-raises it.
            if central.isScanning {
                central.stopScan()
                activeScanServices.removeAll()
            }
            peripheral = retrieved
            retrieved.delegate = self
            central.connect(retrieved)
        case .waitForCurrentConnection:
            break  // didConnect / finishConnect will kick beginWeatherUploadIfReady().
        }
    }

    private func cancelWeatherUpload(_ token: UUID) {
        dispatchPrecondition(condition: .onQueue(queue))
        if weatherUploadToken == token, weatherUploadWaiter != nil {
            failWeatherUpload(.cancelled)
        } else if weatherUploadToken != token {
            // The cancellation handler may beat the registration hop onto this queue.
            cancelledWeatherUploadTokens.insert(token)
        }
    }

    private func armWeatherUploadDeadline(until deadline: Date) {
        weatherUploadDeadline?.cancel()
        let remaining = deadline.timeIntervalSinceNow
        guard remaining > 0 else {
            failWeatherUpload(expiryError())
            return
        }
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            failWeatherUpload(expiryError())
        }
        weatherUploadDeadline = item
        queue.asyncAfter(deadline: .now() + remaining, execute: item)
    }

    /// One absolute deadline from the moment the **connection** came up — the same non-restartable
    /// rule as the read's (§11.3): re-arming per stage (or per leg) would let a slow gated phase
    /// plus a slow CoC send, or a read that shared this link, double the radio hold.
    ///
    /// On a weather-owned connection the base is `weatherOwnedConnectionUpAt`, so a shared
    /// read → upload sequence spends **one** 25 s window, not 8 + 25.
    private func armWeatherUploadConnectedDeadline() {
        dispatchPrecondition(condition: .onQueue(queue))
        weatherUploadConnectedDeadline?.cancel()
        let base = discoveryPolicy.connectionOwnership == .weatherRequest
            ? (weatherOwnedConnectionUpAt ?? .now()) : .now()
        let deadline = weatherUploadConnectedDeadlineAt ?? (base + Self.weatherUploadConnectedBudget)
        weatherUploadConnectedDeadlineAt = deadline
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            failWeatherUpload(expiryError())
        }
        weatherUploadConnectedDeadline = item
        queue.asyncAfter(deadline: deadline, execute: item)
    }

    private func expiryError() -> WeatherUploadError {
        .timedOut
    }

    /// Start the delivery once everything is in place: the intent has a bundle or no-change ACK,
    /// the known peripheral is connected, and the required control characteristic is discovered.
    /// Called from every path that can complete one of those conditions.
    private func beginWeatherUploadIfReady() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard discoveryPolicy.weatherUploadPending, !weatherUploadInFlight else { return }
        guard let payload = weatherUploadPayload, let token = weatherUploadToken
        else { return }  // restoration-adopted, no caller yet
        guard let knownID = discoveryStore.knownPeripheralID(), peripheral?.identifier == knownID,
              peripheral?.state == .connected
        else { return }
        switch payload {
        case .bundle:
            guard characteristics[GATT.objectControl] != nil, characteristics[GATT.psm] != nil
            else { return }
        case .unchanged:
            break
        }
        weatherUploadInFlight = true
        if weatherUploadConnectedAt == nil { weatherUploadConnectedAt = weatherRequestClock.now }
        // Whichever connection this actually landed on decides the receipt's honesty: an upload
        // that *waited out* a foreground connect and then rode it reused a foreground session
        // just as much as one that found it already up.
        weatherUploadReusedForeground = discoveryPolicy.connectionOwnership == .foreground
        armWeatherUploadConnectedDeadline()
        weatherUploadExchange?.cancel()
        weatherUploadExchange = Task { [weak self] in
            await self?.runWeatherDeliveryExchange(payload, token: token)
        }
    }

    private func runWeatherDeliveryExchange(_ delivery: WeatherDelivery, token: UUID) async {
        switch delivery {
        case .bundle(let payload):
            await runWeatherUploadExchange(payload, token: token)
        case .unchanged(let requestID, let retryAfterSeconds):
            await runWeatherUnchangedExchange(
                requestID: requestID, retryAfterSeconds: retryAfterSeconds, token: token
            )
        }
    }

    /// V4 has no unchanged command. Nothing needs to be written when the held revision is already
    /// current; complete the weather leg without inventing an opcode.
    private func runWeatherUnchangedExchange(
        requestID: UInt32, retryAfterSeconds: UInt16, token: UUID
    ) async {
        _ = requestID
        _ = retryAfterSeconds
        guard !Task.isCancelled else { return }
        queue.async { [weak self] in self?.completeWeatherUpload(token: token) }
    }

    /// Weather is an ordinary retaining PUT through the one protocol-v4 client.
    private func runWeatherUploadExchange(_ payload: Data, token: UUID) async {
        do {
            _ = try await performUpload(
                payload: payload, kind: .weather, objectID: nil, displayName: "weather",
                progress: { _ in })
            queue.async { [weak self] in self?.completeWeatherUpload(token: token) }
        } catch let error as DeviceError {
            let failure: WeatherUploadError = switch error {
            case .crcMismatch: .crcMismatch
            case .storageFull: .storageFull
            case .transferRejected: .rejected
            default: .connectionDropped
            }
            queue.async { [weak self] in self?.failWeatherUpload(failure, token: token) }
        } catch {
            queue.async { [weak self] in self?.failWeatherUpload(.connectionDropped, token: token) }
        }
    }

    /// `token` is the attempt the completing exchange belongs to. `nil` means "whatever is
    /// registered" and is only used by paths that *are* the current attempt (deadlines, drops).
    private func completeWeatherUpload(token: UUID? = nil) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard weatherUploadWaiter != nil || discoveryPolicy.weatherUploadPending else { return }
        guard isCurrentWeatherUpload(token) else { return }
        let now = weatherRequestClock.now
        let started = weatherUploadStartedAt ?? now
        let connected = weatherUploadConnectedAt ?? now
        let result = WeatherBundleUpload(
            connectLatency: started.duration(to: connected),
            connectedDuration: connected.duration(to: now),
            reusedForegroundConnection: weatherUploadReusedForeground
        )
        let disconnectOwnedConnection = endWeatherUploadState()
        weatherUploadWaiter?.resume(returning: result)
        weatherUploadWaiter = nil
        print(
            "[OBC BLE weather] delivery complete connect=\(result.connectLatency) "
                + "connected=\(result.connectedDuration) reused=\(result.reusedForegroundConnection)"
        )
        if disconnectOwnedConnection { releaseWeatherOwnedConnection() }
    }

    private func failWeatherUpload(_ error: WeatherUploadError, token: UUID? = nil) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard weatherUploadWaiter != nil || discoveryPolicy.weatherUploadPending else { return }
        guard isCurrentWeatherUpload(token) else { return }
        let disconnectOwnedConnection = endWeatherUploadState()
        weatherUploadWaiter?.resume(throwing: error)
        weatherUploadWaiter = nil
        print("[OBC BLE weather] delivery failed: \(error)")
        if disconnectOwnedConnection {
            releaseWeatherOwnedConnection()
        } else if central.isScanning, discoveryPolicy.scanServices.isEmpty {
            central.stopScan()
        }
    }

    /// A completion from a *superseded* exchange must not resolve the attempt that is registered
    /// now. Attempts are identified by the token minted in `uploadWeatherBundle`; an exchange that
    /// outlived its attempt finds a different (or absent) token and is dropped on the floor.
    private func isCurrentWeatherUpload(_ token: UUID?) -> Bool {
        guard let token else { return true }
        return weatherUploadToken == token
    }

    private func endWeatherUploadState() -> Bool {
        let disconnectOwnedConnection = discoveryPolicy.finishWeatherUpload()
        weatherUploadDeadline?.cancel()
        weatherUploadDeadline = nil
        weatherUploadConnectedDeadline?.cancel()
        weatherUploadConnectedDeadline = nil
        weatherUploadConnectedDeadlineAt = nil
        weatherUploadInFlight = false
        weatherUploadRestoredConnectPending = false
        weatherUploadPayload = nil
        weatherUploadToken = nil
        // Cancel, don't merely forget: a superseded weather task must not keep using the client.
        weatherUploadExchange?.cancel()
        weatherUploadExchange = nil
        discoveryStore.clearWeatherUploadRestoration()
        return disconnectOwnedConnection
    }

    // MARK: DeviceTransport — control plane

    public func deviceInfo() async throws -> DeviceInfo {
        async let fw = readString(GATT.firmwareRevision)
        async let hw = readString(GATT.hardwareRevision)
        async let serial = readString(GATT.serialNumber)
        // v2 read: `version u16 · store_epoch u32 · obcm_version u8 · feature_bits u32` LE (§1).
        // **The length is the version mechanism** — every field is decoded by how
        // much of the read arrived, never by an expected total, which is what let
        // `obcm_version` (E1 / #911) and now the capability word (WX3 / #1188) land
        // without a protocol bump. Four lengths exist: 11 (full), 7 (a firmware
        // predating the capability word), 6 (also predating the obcm byte), 2 (no
        // mounted store). Anything past what we know is ignored, so the next field
        // to be appended will not break this build either.
        //
        // The version field keeps the **lenient prefix** decode (count >= 2 reads
        // the first u16) — that's the v1-peer compat path: a v1 device returns 2
        // bytes, reads as `version = 1`, and takes the #303 mismatch banner. Every
        // trailing field decodes to `nil` when it did not arrive, never to a
        // fabricated `0`: `0` is a legal store epoch, and OBCM `0` would read as
        // "supports OBCM v0" and refuse every real map. V5 (#769) gates
        // `ackRides`/reconcile on a present epoch — a `nil` here is that failed
        // identity read surfaced, not hidden behind a fake value.
        //
        // The capability word needs **all four** of its bytes: 8, 9 or 10 bytes are
        // a broken read of a `u32`, not a smaller capability set, and decoding the
        // bytes that did arrive could claim a feature this device never announced —
        // a phone that then offered weather to a device without it.
        let versionData = try await read(GATT.protocolVersion)
        guard versionData.count >= 2 else { throw DeviceError.readFailed }
        let b = versionData.startIndex
        let version = UInt16(versionData[b]) | (UInt16(versionData[b + 1]) << 8)
        let storeEpoch: UInt32? = versionData.count >= 6
            ? UInt32(versionData[b + 2]) | (UInt32(versionData[b + 3]) << 8)
                | (UInt32(versionData[b + 4]) << 16) | (UInt32(versionData[b + 5]) << 24)
            : nil
        let obcmVersion: UInt8? = versionData.count >= 7 ? versionData[b + 6] : nil
        let featureBits: UInt32? = versionData.count >= 11
            ? UInt32(versionData[b + 7]) | (UInt32(versionData[b + 8]) << 8)
                | (UInt32(versionData[b + 9]) << 16) | (UInt32(versionData[b + 10]) << 24)
            : nil
        let name = await currentPeripheralName() ?? "OBC"
        let serialValue = try await serial
        let storeID: String?
        if version == OBCProtocol.version {
            storeID = try await transferClient.storeID().description
        } else {
            storeID = nil
        }
        let info = DeviceInfo(
            name: name, firmwareVersion: try await fw, hardwareVersion: try await hw,
            serial: serialValue, protocolVersion: version, storeEpoch: storeEpoch, storeID: storeID,
            obcmVersion: obcmVersion, featureBits: featureBits
        )
        return info
    }

    public func readConfig() async throws -> DeviceConfig {
        try ConfigObjectCodec.decode(try await read(GATT.config))
    }

    public func writeConfig(_ config: DeviceConfig) async throws {
        try await write(ConfigObjectCodec.encode(config), to: GATT.config)
    }

    public func readDiagnostics() async throws -> Data {
        // Protocol v4 has no diagnostics kind. Keep the capability's old surface fail-closed
        // until a future registered kind replaces it.
        throw DeviceError.readFailed
    }

    public func deleteRoute(_ id: DeviceObjectID) async throws {
        try await remove(kind: .route, id: id)
    }

    public func ackRides(_ ids: [RideID]) async throws {
        // Protocol v4 has no possession mutation. The library already owns downloaded bytes;
        // keeping this compatibility capability as a no-op avoids inventing an unregistered frame.
    }

    public func setClock(_ sample: WallClockSample) async throws -> ClockSyncOutcome {
        .unsupported
    }

    public func setRouteRetention(
        _ id: DeviceObjectID, _ retention: Retention
    ) async throws -> RetentionWriteOutcome {
        .unsupported
    }

    public func listRoutes() async throws -> [RouteCatalogEntry] {
        // This catalog is reconcile-only: identity + CRC are its proof, and it never feeds route
        // rows. Protocol v4 already carries both in LIST. Downloading every OBCR merely to rebuild
        // legacy display fields creates an N+1 transfer storm (249 GETs on the flat-store bench
        // card) and blocks a foreground PUT behind the TransferClient's operation gate.
        try await headEntries(kind: .route).map { entry in
            RouteCatalogEntry(
                id: DeviceObjectID(entry.objectID.rawValue), name: entry.displayName,
                distanceMeters: 0, elevationGainMeters: 0,
                pointCount: 0, crc32: entry.payloadCRC32)
        }
    }

    public func listRides() async throws -> RideCatalog {
        // LIST is the catalog and its StoreId scopes every ride id minted here.
        let catalog: (storeID: StoreID, entries: [CatalogEntry])
        do { catalog = try await transferClient.catalog(kind: .ride) }
        catch { throw deviceError(for: error) }
        let scope = LibraryScope(
            serial: try await readString(GATT.serialNumber), storeID: catalog.storeID.description)
        var rides: [RideSummary] = []
        for entry in catalog.entries where !entry.flags.contains(.retained)
            && !entry.flags.contains(.reserved) && !entry.flags.contains(.recording) {
            let id = RideID(
                deviceObjectID: DeviceObjectID(entry.objectID.rawValue), scope: scope)
            // FS8's footer is not frozen yet. Keep the fielded ride decoder behind the new GET
            // path; replacing this decode is deliberately outside FS10's iOS half.
            rides.append(try RideObjectCodec.decode(try await download(entry), id: id).summary)
        }
        return RideCatalog(rides: rides, hiddenRideCount: 0)
    }

    public func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
        // Pinned by S0 as "download the route object" (spec §7.1): the stored OBCR
        // v2 blob, decoded app-side for the waypoints + elevation profile — one
        // layout, one truth.
        let decoded = try RouteObjectCodec.decode(try await download(kind: .route, id: id))
        // Header totals are exact (from the producer's raw-point pass); the profile
        // + max grade come from the stored geometry, as E2 renders them.
        let geometry = RouteStats.compute(from: decoded.points)
        // A device-stored object has no library identity — the summary rides
        // under a placeholder id nothing keys on (the detail screen for library
        // routes never comes through here, #289).
        let summary = RouteSummary(
            id: RouteID("device-\(id.raw)"),
            name: decoded.name,
            distanceMeters: Double(decoded.totalDistanceMeters),
            elevationGainMeters: Double(decoded.totalAscentMeters),
            estimatedDuration: geometry.estimatedDuration,
            pointCount: decoded.points.count,
            trackPreview: TrackPreview.normalizing(decoded.points.map(\.coordinate))
        )
        return RouteDetail(
            summary: summary,
            waypoints: decoded.waypoints,
            elevationProfile: geometry.elevationProfile,
            maxGradePercent: geometry.maxGradePercent
        )
    }

    public func rideDetail(_ id: RideID) async throws -> RideDetail {
        // A ride's detail decodes from its downloaded ride object (B7/A7); the
        // synced library copy answers this screen today.
        throw DeviceError.readFailed
    }

    public func listTrips() async throws -> [TripCatalogEntry] {
        // Like routes, this is badge/reconcile input. LIST already carries every field that input
        // consumes; stage details are fetched only when a caller explicitly downloads the trip.
        try await headEntries(kind: .trip).map { entry in
            TripCatalogEntry(
                id: DeviceObjectID(entry.objectID.rawValue), name: entry.displayName,
                distanceMeters: 0, elevationGainMeters: 0,
                stageCount: 0, crc32: entry.payloadCRC32)
        }
    }

    public func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded {
        // "Download the trip object" (spec §7.7) — the stored trip blob, decoded
        // app-side for its name + stage ids. Reconcile falls back to it only when
        // the `tripList` `crc32` can't confirm the fingerprint.
        try TripObjectCodec.decode(try await download(kind: .trip, id: id))
    }

    public func deleteTrip(_ id: DeviceObjectID) async throws {
        try await remove(kind: .trip, id: id)
    }

    private func headEntries(kind: ObjectKind) async throws -> [CatalogEntry] {
        do {
            return try await transferClient.list(kind: kind).filter {
                !$0.flags.contains(.retained) && !$0.flags.contains(.reserved)
            }
        } catch { throw deviceError(for: error) }
    }

    private func headEntry(kind: ObjectKind, id: DeviceObjectID) async throws -> CatalogEntry {
        guard let entry = try await headEntries(kind: kind).first(where: { $0.objectID.rawValue == id.raw })
        else { throw DeviceError.readFailed }
        return entry
    }

    private func download(_ entry: CatalogEntry) async throws -> Data {
        do {
            return try await transferClient.get(
                objectID: entry.objectID, revision: entry.revision).payload
        } catch { throw deviceError(for: error) }
    }

    fileprivate func download(kind: ObjectKind, id: DeviceObjectID) async throws -> Data {
        try await download(headEntry(kind: kind, id: id))
    }

    private func remove(kind: ObjectKind, id: DeviceObjectID) async throws {
        let entry = try await headEntry(kind: kind, id: id)
        do { _ = try await transferClient.remove(objectID: entry.objectID, expectedRevision: entry.revision) }
        catch { throw deviceError(for: error) }
    }

    private func deviceError(for error: Error) -> DeviceError {
        if let error = error as? DeviceError { return error }
        if error is TransferLinkLost { return .transferDropped }
        if let error = error as? TransferClientError {
            switch error {
            case .checksumMismatch: return .crcMismatch
            case .storeChanged, .outcomeNotCommitted: return .transferDropped
            default: return .transferRejected
            }
        }
        if let wire = error as? WireError, case .remote(let body) = wire {
            switch body.code {
            case .noSpace: return .storageFull
            case .checksumFailure: return .crcMismatch
            case .cancelled: return .transferDropped
            case .notFound: return .readFailed
            default: return .transferRejected
            }
        }
        return .transferRejected
    }

    // MARK: DeviceTransport — data plane

    /// The shared upload service for route, trip, and firmware objects. It borrows this
    /// transport's queue-confined connection/channel engine; it does not own another manager,
    /// queue, or channel.
    private enum UploadService {
        static func start(
            over transport: BLETransport, payload: Data, kind: ObjectKind,
            objectID: DeviceObjectID?, displayName: String, reportsAssignedID: Bool
        ) -> TransferHandle {
            guard !payload.isEmpty else {
                return .immediatelyFinished(.failed(.transferRejected))
            }
            let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
            let outcome = AsyncPromise<TransferOutcome>()
            let assignedID = AsyncPromise<DeviceObjectID?>()
            let runner = V4UploadRunner(
                transport: transport, payload: payload, kind: kind, objectID: objectID,
                displayName: displayName,
                progress: continuation, outcome: outcome, assignedID: assignedID
            )
            Task { await runner.start() }
            return TransferHandle(
                progress: stream, outcome: outcome,
                assignedObjectID: reportsAssignedID ? assignedID : nil,
                onCancel: { Task { await runner.cancel() } },
                onResume: {}
            )
        }
    }

    public func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        // A fresh upload sends ObjectId zero and keeps the id assigned by the PUT result.
        // Re-uploading an edited route names its stored id and exact revision.
        UploadService.start(
            over: self, payload: route.payload, kind: .route,
            objectID: route.targetObjectID, displayName: route.summary.name,
            reportsAssignedID: true
        )
    }

    public func uploadTrip(_ trip: TripBlob) -> TransferHandle {
        // The trip sibling of `uploadRoute`, using the same v4 PUT path. It is uploaded last in a
        // whole-trip push after its route stages.
        UploadService.start(
            over: self, payload: trip.payload, kind: .trip,
            objectID: trip.targetObjectID, displayName: trip.name,
            reportsAssignedID: true
        )
    }

    public func uploadFirmware(_ container: Data) -> TransferHandle {
        // The whole OBCU container is an ordinary update-kind create. Staging never installs;
        // `installFirmware` performs the separate ARM request.
        UploadService.start(
            over: self, payload: container, kind: .update,
            objectID: nil, displayName: "UPDATE.BIN", reportsAssignedID: false
        )
    }

    public func installFirmware() async throws -> FirmwareInstallResult {
        guard let package = try await headEntries(kind: .update).max(by: { $0.revision < $1.revision })
        else { return .noStaged }
        do {
            _ = try await transferClient.arm(
                packageObjectID: package.objectID, expectedRevision: package.revision)
            return .accepted
        } catch WireError.remote(let body) {
            switch body.code {
            case .notFound: return .noStaged
            case .busy: return .busy
            case .unsupported: return .unsupported
            case .rejected: return .rejected
            default: return .rejected
            }
        } catch {
            throw deviceError(for: error)
        }
    }

    fileprivate func performUpload(
        payload: Data, kind: ObjectKind, objectID: DeviceObjectID?, displayName: String,
        progress: @escaping @Sendable (TransferProgress) -> Void
    ) async throws -> DeviceObjectID {
        var target = objectID
        if target == nil, kind == .update || kind == .weather {
            target = try await headEntries(kind: kind).max(by: { $0.revision < $1.revision })
                .map { DeviceObjectID($0.objectID.rawValue) }
        }
        let expected: Revision?
        if let target {
            expected = try await headEntry(kind: kind, id: target).revision
        } else {
            expected = nil
        }
        do {
            let result = try await transferClient.put(
                payload, objectID: target.map { ObjectID(rawValue: $0.raw) },
                expectedRevision: expected, kind: kind,
                retainPrevious: kind == .weather, displayName: displayName
            ) { done, total in
                progress(TransferProgress(bytesDone: done, total: total))
            }
            return DeviceObjectID(result.objectID.rawValue)
        } catch { throw deviceError(for: error) }
    }

    public func forgetBond() async throws {
        // The opaque CoreBluetooth identifier is useful only while this bond is trusted. Clear it
        // even when the best-effort device command fails, so restoration can never act on a device
        // the app has locally forgotten.
        defer {
            discoveryStore.clearKnownPeripheralID()
            discoveryStore.clearWeatherRestoration()
            discoveryStore.clearWeatherUploadRestoration()
        }
        // Protocol v4 registers no remote bond mutation. Clearing the companion's trusted
        // CoreBluetooth identity is the entire app-side operation.
    }

    public func downloadRides(_ ids: [RideID]) -> RideDownload {
        // Real path (A7): one ride-object download per id, persisted ride-by-ride,
        // so a drop keeps what landed and "resume" re-requests only the missing
        // rides (whole rides are the batch's elementary unit — spec §1 principle 4).
        guard !ids.isEmpty else { return .finished() }                      // H9
        if stateMulticast.value == .disconnected {
            return .finished(.failed(.notConnected))                        // H4
        }
        // Resolve every id's device object id up front: ids on this plane come
        // from `listRides()`, which mints them via `RideID(deviceObjectID:)`, so
        // a non-device id is a caller bug — fail the batch loudly rather than
        // skip it silently (#359).
        var requests: [(id: RideID, objectID: DeviceObjectID)] = []
        for id in ids {
            guard let objectID = id.deviceObjectID else {
                return .finished(.failed(.transferRejected))
            }
            requests.append((id, objectID))
        }
        let (rideStream, rideContinuation) = AsyncThrowingStream<DownloadedRide, Error>.makeStream()
        let (progressStream, progressContinuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let runner = RideDownloadRunner(
            transport: self, requests: requests, rides: rideContinuation,
            progress: progressContinuation, outcome: outcome
        )
        Task { await runner.start() }
        let handle = TransferHandle(
            progress: progressStream, outcome: outcome,
            onCancel: { Task { await runner.cancel() } },
            onResume: {}
        )
        return RideDownload(handle: handle, rides: rideStream)
    }

    // MARK: Physical CoC access

    /// The live CoC channel, opening it if necessary.
    fileprivate func readyChannel() async throws -> BLEChannel {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<BLEChannel, Error>) in
            queue.async { [self] in
                if let bleChannel, byteChannel?.isOpen == true {
                    cont.resume(returning: bleChannel)
                    return
                }
                guard let peripheral, let psm = characteristics[GATT.psm] else {
                    cont.resume(throwing: DeviceError.notConnected)
                    return
                }
                channelWaiters.append(cont)
                if !openingChannel {
                    openingChannel = true
                    armChannelWatchdog()
                    byteChannel = nil
                    bleChannel = nil
                    peripheral.readValue(for: psm)  // → PSM update → openL2CAPChannel → didOpen
                }
            }
        }
    }

    // MARK: Connect flow (queue-confined)

    private func startConnectIfReady() {
        guard discoveryPolicy.hasIntent else { return }
        guard discoveryPolicy.phase == .scanning || discoveryPolicy.phase == .idle else { return }
        switch central.state {
        case .poweredOn:
            let services = discoveryPolicy.scanServices
            guard !services.isEmpty else { return }
            // A weather-only scan exists to wake on the *known bonded* device; with nothing bonded
            // there is nothing to wake for, and the standing watch must not burn radio on it.
            if services == [.weatherRequest], !discoveryPolicy.weatherRequestPending,
               discoveryStore.knownPeripheralID() == nil {
                return
            }
            discoveryPolicy.noteScanning()  // the watch's scan has no request* call to raise the phase
            if discoveryPolicy.foregroundRequested { stateMulticast.send(.connecting) }
            if central.isScanning, activeScanServices == services { return }
            if central.isScanning, activeScanServices != services { central.stopScan() }
            activeScanServices = services
            let cbServices = services.map {
                switch $0 {
                case .control: GATT.obcControlService
                case .weatherRequest: GATT.weatherRequestService
                }
            }
            // For an already-authenticated foreground peer, its opaque identifier is the filter.
            // An unfiltered scan is a recovery path for firmware/app UUID skew and CoreBluetooth
            // advertisement-dictionary quirks; `discovered` rejects every other peripheral. The
            // standing background watch remains UUID-filtered, as iOS requires for wake-ups.
            let scanServices: [CBUUID]? =
                discoveryPolicy.foregroundRequested && discoveryStore.knownPeripheralID() != nil
                ? nil : cbServices
            central.scanForPeripherals(withServices: scanServices)
        case .poweredOff:
            failRadioUnavailable(.bluetoothUnavailable(.poweredOff))
        case .unauthorized:
            failRadioUnavailable(.bluetoothUnavailable(.unauthorized))
        case .unsupported:
            failRadioUnavailable(.bluetoothUnavailable(.unsupported))
        default:
            break  // .resetting / .unknown → wait for the next state update
        }
    }

    private func failRadioUnavailable(_ error: DeviceError) {
        if discoveryPolicy.weatherRequestPending { failWeatherRequest(.bluetoothUnavailable) }
        if discoveryPolicy.weatherUploadPending || weatherUploadWaiter != nil {
            failWeatherUpload(.bluetoothUnavailable)
        }
        if discoverContinuation != nil {
            failDiscover(error)
        } else {
            _ = discoveryPolicy.cancelForeground()
            stateMulticast.send(.disconnected)
        }
        activeScanServices.removeAll()
        if central.isScanning { central.stopScan() }
    }

    /// Kick off phase 2 (#297): arm the gated notifies then read the PSM to open
    /// the CoC — the first gated op is what raises the passkey sheet. Drives both
    /// the explicit `authenticate()` call (fresh pair) and the auto-resume after a
    /// background reconnect (bonded, no continuation waiting).
    private func beginAuthenticate() {
        guard let peripheral, let psm = characteristics[GATT.psm] else {
            failAuthenticate(.notConnected)
            return
        }
        // Protocol v4 answers every object request on the one indicated control characteristic.
        if let objectControl = characteristics[GATT.objectControl] {
            peripheral.setNotifyValue(true, for: objectControl)
        }
        // #753: the gated retry can begin with the CoC already up — a CCCD write
        // failed and resolved attempt 1 as retryable while attempt 1's PSM read
        // stayed in flight on the serialized ATT bearer, and that read then
        // succeeded and opened the channel *during* the retry beat (its
        // `finishConnect` had no continuation to resolve). The phase's goal
        // state is reached — notifies re-armed above, channel open — so resolve
        // the parked authenticate now instead of waiting on a `didOpen` that
        // already fired.
        if bleChannel != nil, byteChannel?.isOpen == true {
            finishConnect()
            return
        }
        if bleChannel == nil, !openingChannel {
            openingChannel = true
            // The gated PSM read is what raises the passkey sheet on a fresh
            // pair, so with an `authenticate()` parked the budget must cover
            // the rider typing the passkey. A bonded background reconnect (no
            // continuation) re-encrypts silently — tight budget.
            armChannelWatchdog(after: authenticateContinuation != nil ? Self.pairingTimeout : Self.phaseTimeout)
            peripheral.readValue(for: psm)  // → PSM update → openL2CAPChannel → didOpen
        } else if openingChannel, channelWatchdog == nil {
            // #753: a CCCD-triggered retryable resolve disarms the watchdog while
            // attempt 1's PSM read keeps `openingChannel` latched (its response is
            // still owed on the serialized ATT bearer), so this retry entry can't
            // re-issue the read — re-watch the in-flight open instead, or a read
            // that never resolves would park the retry forever (#302's wedge,
            // unwatched).
            armChannelWatchdog(after: authenticateContinuation != nil ? Self.pairingTimeout : Self.phaseTimeout)
        }
    }

    /// Arm the GATT-discovery watchdog (start of `discoverServices`). If it fires,
    /// discovery never completed — an empty/partial DB — so fail a parked
    /// `discover()` and drop the link; a bonded reconnect then retries clean.
    private func armDiscoveryWatchdog() {
        discoveryWatchdog?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.discoveryWatchdog = nil
            if self.discoveryPolicy.connectionOwnership == .weatherRequest {
                if self.discoveryPolicy.weatherRequestPending { self.failWeatherRequest(.timedOut) }
                if self.discoveryPolicy.weatherUploadPending { self.failWeatherUpload(.timedOut) }
            } else if self.discoverContinuation != nil {
                self.failDiscover(.deviceNotFound)
            }
            if let peripheral = self.peripheral { self.central.cancelPeripheralConnection(peripheral) }
        }
        discoveryWatchdog = item
        queue.asyncAfter(deadline: .now() + Self.phaseTimeout, execute: item)
    }

    private func disarmDiscoveryWatchdog() {
        discoveryWatchdog?.cancel()
        discoveryWatchdog = nil
    }

    /// Arm the CoC-open watchdog (when `openingChannel` is raised). If it fires,
    /// the open stalled (PSM read or `openL2CAPChannel` never yielded `didOpen`):
    /// clear the latch, fail the parked opens, and unwind a pending authenticate —
    /// the next transfer re-opens from scratch instead of parking forever.
    /// `timeout` is `phaseTimeout` except across a fresh pair's PSM read, where
    /// the passkey sheet makes the phase human-paced (`pairingTimeout`).
    private func armChannelWatchdog(after timeout: DispatchTimeInterval = BLETransport.phaseTimeout) {
        channelWatchdog?.cancel()
        let item = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.channelWatchdog = nil
            self.openingChannel = false
            let waiters = self.channelWaiters
            self.channelWaiters.removeAll()
            for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
            if self.authenticateContinuation != nil { self.failAuthenticate(.channelOpenFailed) }
        }
        channelWatchdog = item
        queue.asyncAfter(deadline: .now() + timeout, execute: item)
    }

    private func disarmChannelWatchdog() {
        channelWatchdog?.cancel()
        channelWatchdog = nil
    }

    /// Phase 1 failed (radio, scan, GATT discovery) — the link never came up.
    private func failDiscover(_ error: DeviceError) {
        disarmDiscoveryWatchdog()
        _ = discoveryPolicy.cancelForeground()
        stateMulticast.send(.disconnected)
        discoverContinuation?.resume(throwing: error)
        discoverContinuation = nil
    }

    private func failConnectionSetup() {
        if discoveryPolicy.connectionOwnership == .weatherRequest {
            if discoveryPolicy.weatherRequestPending { failWeatherRequest(.readFailed) }
            if discoveryPolicy.weatherUploadPending { failWeatherUpload(.connectionDropped) }
        } else if discoverContinuation != nil {
            failDiscover(.notConnected)
        }
        if let peripheral { central.cancelPeripheralConnection(peripheral) }
    }

    /// Phase 2 failed (declined passkey / refused encryption / CoC open) — tear the
    /// intent down so a background reconnect doesn't spin on a bond that won't take.
    private func failAuthenticate(_ error: DeviceError) {
        disarmChannelWatchdog()
        awaitingGatedRetry = false
        _ = discoveryPolicy.cancelForeground()
        stateMulticast.send(.disconnected)
        authenticateContinuation?.resume(throwing: error)
        authenticateContinuation = nil
    }

    /// #753: resolve a parked fresh-pair `authenticate()` as *retryable* — throw
    /// `GatedPairingWindowError` so `authenticate()` runs the gated phase once
    /// more on this same bonded link. Unlike `failAuthenticate`, it leaves
    /// `wantsConnect` and the (still-up) link intact and publishes no
    /// `.disconnected`; it flags the beat with `awaitingGatedRetry` so a drop in
    /// the window (before the retry parks its continuation) is terminal.
    private func resolveAuthenticateRetryable() {
        disarmChannelWatchdog()
        awaitingGatedRetry = true
        authenticateContinuation?.resume(throwing: GatedPairingWindowError())
        authenticateContinuation = nil
    }

    /// #753: the single retry also failed with the pairing-window error, so the
    /// link may still be up with `wantsConnect` set (the retryable resolve left it
    /// intact for the retry). Drop the half-bonded link and the intent so the
    /// reconnect loop can't re-raise the passkey behind D5, and a fresh D5 "Try
    /// again" can re-discover a disconnected peripheral.
    private func teardownAfterFailedRetry() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                awaitingGatedRetry = false
                _ = discoveryPolicy.cancelForeground()
                if let peripheral { central.cancelPeripheralConnection(peripheral) }
                if stateMulticast.value != .disconnected { stateMulticast.send(.disconnected) }
                cont.resume()
            }
        }
    }

    /// Whether an ATT/CB error means the encrypted, LESC-authenticated link the
    /// gated characteristics require (A8) wasn't established — the passkey was
    /// declined/wrong or the bond was refused. Distinguishes a real pairing
    /// failure from an ordinary read/open error so the launch flow can show the
    /// right D5 copy.
    private static func isAuthError(_ error: Error?) -> Bool {
        if let att = error as? CBATTError {
            switch att.code {
            case .insufficientAuthentication, .insufficientEncryption, .insufficientAuthorization:
                return true
            default:
                return false
            }
        }
        if let cb = error as? CBError {
            switch cb.code {
            case .encryptionTimedOut, .peerRemovedPairingInformation:
                return true
            default:
                return false
            }
        }
        return false
    }

    /// #753 — the one shared retry proxy for a failed *gated op* (the `objectControl`
    /// CCCD write or the PSM read), used by both delegate
    /// branches so they can't drift: retryable only when the failure is
    /// auth-class (`isAuthError`) **and** the peripheral is still connected
    /// **and** a fresh-pair `authenticate()` is parked. Auth-class while still
    /// connected is the conservative "SMP pairing visibly completed, firmware
    /// momentarily refused" evidence (see `GatedPairingWindowError`); everything
    /// else — a decline that drops the link, a non-auth failure, a background
    /// re-arm with no authenticate pending — stays terminal, exactly as before.
    /// `nil` (the op succeeded) is never retryable. Internal, not private, so
    /// the mapping is unit-testable without a radio.
    static func isRetryableGatedFailure(
        _ error: Error?, peripheralConnected: Bool, authenticatePending: Bool
    ) -> Bool {
        authenticatePending && peripheralConnected && isAuthError(error)
    }

    private func finishConnect() {
        // An **ephemeral weather connection** (WX9) reaches here when its gated phase opens the
        // CoC for the upload leg. It must never read as the foreground link: publishing
        // `.connected` would wake every foreground observer of a session the user does not have,
        // and there is no authenticate continuation to resolve. The channel waiters were already
        // resumed by `didOpen`; just keep the weather work moving.
        if discoveryPolicy.connectionOwnership == .weatherRequest, !discoveryPolicy.foregroundRequested {
            if let peripheral { discoveryStore.saveKnownPeripheralID(peripheral.identifier) }
            if discoveryPolicy.weatherRequestPending { beginWeatherRequestReadIfReady() }
            if discoveryPolicy.weatherUploadPending { beginWeatherUploadIfReady() }
            return
        }
        // Only announce an actual transition (#302): a mid-session CoC reopen
        // (after a canceled-transfer `teardownChannel`) re-enters here, but the
        // link never left `.connected` — re-sending would re-fire edge-triggered
        // observers. The authenticate continuation still resolves unconditionally
        // (a fresh `authenticate()` completes here regardless of the state edge).
        if stateMulticast.value != .connected { stateMulticast.send(.connected) }
        if let peripheral {
            // Reaching the authenticated CoC proves this opaque CoreBluetooth identifier belongs to
            // the trusted device; only then may a future restored background intent bind to it.
            discoveryStore.saveKnownPeripheralID(peripheral.identifier)
        }
        awaitingGatedRetry = false
        authenticateContinuation?.resume()
        authenticateContinuation = nil
        if discoveryPolicy.weatherRequestPending { beginWeatherRequestReadIfReady() }
        if discoveryPolicy.weatherUploadPending { beginWeatherUploadIfReady() }
    }

    /// The link is gone: every parked continuation must resolve (a leaked
    /// `CheckedContinuation` hangs its caller forever), and buffered notifications
    /// from the dead link are dropped (a new connection re-announces).
    private func failAllPending() {
        let reads = pendingReads.values.flatMap { $0 }
        pendingReads.removeAll()
        let writes = pendingWrites.values.flatMap { $0 }
        pendingWrites.removeAll()
        let channels = channelWaiters
        channelWaiters.removeAll()
        let objectControls = objectControlWaiters
        objectControlWaiters.removeAll()
        pendingObjectControlRecords.removeAll()
        openingChannel = false
        disarmChannelWatchdog()
        for cont in reads { cont.resume(throwing: DeviceError.notConnected) }
        for cont in writes { cont.resume(throwing: DeviceError.notConnected) }
        for waiter in objectControls { waiter.resume(throwing: TransferLinkLost()) }
        for cont in channels { cont.resume(throwing: DeviceError.notConnected) }
    }

    // MARK: Async ↔ delegate bridges (queue-confined)

    private func read(_ uuid: CBUUID) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: DeviceError.notConnected)
                    return
                }
                pendingReads[uuid, default: []].append(cont)
                peripheral.readValue(for: characteristic)
            }
        }
    }

    private func readString(_ uuid: CBUUID) async throws -> String {
        String(decoding: try await read(uuid), as: UTF8.self)
    }

    fileprivate func write(_ data: Data, to uuid: CBUUID) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: DeviceError.notConnected)
                    return
                }
                pendingWrites[uuid, default: []].append(cont)
                peripheral.writeValue(data, for: characteristic, type: .withResponse)
            }
        }
    }

    private func currentPeripheralName() async -> String? {
        await withCheckedContinuation { (cont: CheckedContinuation<String?, Never>) in
            queue.async { [self] in cont.resume(returning: peripheral?.name) }
        }
    }
}

// MARK: - The upload runner

/// One protocol-v4 upload task. TransferClient owns the live request and its STATUS/LIST
/// reconciliation, so this adapter has no restart/resume state machine.
private actor V4UploadRunner {
    private let transport: BLETransport
    private let payload: Data
    private let kind: ObjectKind
    private let objectID: DeviceObjectID?
    private let displayName: String
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>
    private let assignedID: AsyncPromise<DeviceObjectID?>
    private var attempt: Task<Void, Never>?
    private var started = false

    init(
        transport: BLETransport, payload: Data, kind: ObjectKind,
        objectID: DeviceObjectID?, displayName: String,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>, assignedID: AsyncPromise<DeviceObjectID?>
    ) {
        self.transport = transport
        self.payload = payload
        self.kind = kind
        self.objectID = objectID
        self.displayName = displayName
        self.progress = progress
        self.outcome = outcome
        self.assignedID = assignedID
    }

    func start() {
        guard !started else { return }
        started = true
        attempt = Task {
            await runAttempt()
            attempt = nil
        }
    }

    func cancel() async {
        attempt?.cancel()
        finish(.canceled)
    }

    private func runAttempt() async {
        do {
            let ticks = progress
            let id = try await transport.performUpload(
                payload: payload, kind: kind, objectID: objectID, displayName: displayName
            ) { ticks.yield($0) }
            assignedID.fulfill(id)
            finish(.completed)
        } catch is CancellationError {
            finish(.canceled)
        } catch let error as DeviceError {
            finish(.failed(error))
        } catch {
            finish(.failed(.transferRejected))
        }
    }

    private func finish(_ terminal: TransferOutcome) {
        guard outcome.current == nil else { return }
        progress.finish()
        outcome.fulfill(terminal)
        if terminal != .completed { assignedID.fulfill(nil) }
    }
}

/// Drives a ride-sync batch through the same protocol-v4 client. A broken GET restarts itself on
/// the restored link; the batch keeps no resume cursor or reconciliation cache.
private actor RideDownloadRunner {
    private let transport: BLETransport
    /// Each requested ride with its device object id, resolved (and validated)
    /// by `downloadRides` before the runner exists.
    private let requests: [(id: RideID, objectID: DeviceObjectID)]
    private let rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>
    private var attempt: Task<Void, Never>?
    private var started = false
    private var finished = false

    init(
        transport: BLETransport, requests: [(id: RideID, objectID: DeviceObjectID)],
        rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>
    ) {
        self.transport = transport
        self.requests = requests
        self.rides = rides
        self.progress = progress
        self.outcome = outcome
    }

    func start() {
        guard !started else { return }
        started = true
        attempt = Task {
            await runAttempt()
            attempt = nil
        }
    }

    func cancel() async {
        attempt?.cancel()
        finish(.canceled)
    }

    private func runAttempt() async {
        guard !finished else { return }
        do {
            for (index, request) in requests.enumerated() {
                try Task.checkCancellation()
                let payload = try await transport.download(kind: .ride, id: request.objectID)
                rides.yield(DownloadedRide(id: request.id, payload: payload))
                progress.yield(TransferProgress(bytesDone: index + 1, total: requests.count))
            }
            finish(.completed)
        } catch is CancellationError {
            finish(.canceled)
        } catch DeviceError.crcMismatch {
            // A corrupt ride object is a hard, non-retryable failure (the bytes on
            // the card are bad) — surface it into the stream and end the batch.
            if !finished {
                finished = true
                progress.finish()
                rides.finish(throwing: DeviceError.crcMismatch)
                outcome.fulfill(.failed(.crcMismatch))
            }
        } catch {
            finish(.failed((error as? DeviceError) ?? .transferDropped))
        }
    }

    private func finish(_ terminal: TransferOutcome) {
        guard !finished else { return }
        finished = true
        progress.finish()
        rides.finish()
        outcome.fulfill(terminal)
    }
}

// MARK: - Protocol-v4 physical link

extension BLETransport: TransferLink {
    public nonisolated var maximumStreamPayload: Int {
        BLEChannel.defaultChunkSize - FlatStoreV4.streamHeaderLength
    }

    public func sendControlRecord(_ record: Data) async throws {
        do {
            _ = try ControlFrame(decoding: record, direction: .request)
            try await write(record, to: GATT.objectControl)
        } catch is WireError {
            throw DeviceError.writeFailed
        } catch {
            throw TransferLinkLost()
        }
    }

    public func receiveControlRecord() async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            queue.async { [self] in
                if !pendingObjectControlRecords.isEmpty {
                    continuation.resume(returning: pendingObjectControlRecords.removeFirst())
                } else if peripheral?.state == .connected {
                    objectControlWaiters.append(continuation)
                } else {
                    continuation.resume(throwing: TransferLinkLost())
                }
            }
        }
    }

    public func sendStreamRecord(_ record: Data) async throws {
        do {
            let channel = try await readyChannel()
            try await channel.sendRecord(record)
        }
        catch is CancellationError { throw CancellationError() }
        catch { throw TransferLinkLost() }
    }

    public func receiveStreamRecord() async throws -> Data {
        do {
            let channel = try await readyChannel()
            return try await withTaskCancellationHandler {
                try await channel.receiveRecord()
            } onCancel: {
                channel.cancelReceive()
            }
        }
        catch is CancellationError { throw CancellationError() }
        catch { throw TransferLinkLost() }
    }

    public func cancelStreamReceive() async {
        queue.sync { bleChannel?.cancelReceive() }
    }

    public func restore() async throws {
        if stateMulticast.value != .connected {
            enum RestoreBeat: Sendable { case connected, timedOut }
            let states = state
            let beat = await withTaskGroup(of: RestoreBeat.self) { group in
                group.addTask {
                    for await value in states where value == .connected { return .connected }
                    return .timedOut
                }
                group.addTask {
                    try? await Task.sleep(for: .seconds(20))
                    return .timedOut
                }
                let first = await group.next() ?? .timedOut
                group.cancelAll()
                return first
            }
            guard case .connected = beat else { throw TransferLinkLost() }
        }
        do { _ = try await readyChannel() }
        catch { throw TransferLinkLost() }
    }
}

// MARK: - CBCentralManagerDelegate

extension BLETransport: CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        startConnectIfReady()
        // A restoration-adopted upload intent that could not issue its connect yet (this delegate
        // runs *after* `willRestoreState`, so that call saw `.unknown` and CoreBluetooth ignored
        // it) gets its one chance here — before the generic re-entry below, which would find the
        // policy already `.connecting` and wait for a connect nobody ever issued.
        resumeRestoredWeatherUploadConnect()
        // An upload registered before the manager reached .poweredOn (the engine's launch-time
        // resume) parks with no scan to revive it — re-enter its connect path now. The function
        // handles every radio state itself, hard-off included.
        startWeatherUploadConnectIfReady()
    }

    /// Issue (or adopt) the restored upload connect once the radio is actually available.
    private func resumeRestoredWeatherUploadConnect() {
        dispatchPrecondition(condition: .onQueue(queue))
        guard weatherUploadRestoredConnectPending, discoveryPolicy.weatherUploadPending,
              let restored = peripheral
        else { return }
        switch central.state {
        case .poweredOn:
            weatherUploadRestoredConnectPending = false
            switch restored.state {
            case .connected: centralManager(central, didConnect: restored)
            case .connecting: break  // CoreBluetooth will deliver didConnect/didFail.
            default: central.connect(restored)
            }
        case .poweredOff, .unauthorized, .unsupported:
            weatherUploadRestoredConnectPending = false
            failWeatherUpload(.bluetoothUnavailable)
        default:
            break  // .unknown / .resetting — the next state update re-enters here.
        }
    }

    public func centralManager(_ central: CBCentralManager, willRestoreState dict: [String: Any]) {
        guard let knownID = discoveryStore.knownPeripheralID() else {
            discoveryStore.clearWeatherRestoration()
            discoveryStore.clearWeatherUploadRestoration()
            return
        }
        // The upload leg's restoration (WX9): a pending **direct connect** — no scan — relaunched
        // the process. Adopt the known peripheral and hold the connection; the payload arrives when
        // the job engine (resumed by the app launch this relaunch *is*) calls
        // `uploadWeatherBundle` from its `bundleReady` checkpoint. The persisted deadline bounds
        // the wait either way, so an engine that never comes back cannot leak a held connection.
        if let uploadDeadline = discoveryStore.weatherUploadRestorationDeadline() {
            if uploadDeadline > Date() {
                let restoredPeripherals =
                    dict[CBCentralManagerRestoredStatePeripheralsKey] as? [CBPeripheral] ?? []
                let restoredIDs = Set(restoredPeripherals.map(\.identifier))
                if let restoredID = discoveryPolicy.restoreWeatherUpload(
                    restoredPeripheralIDs: restoredIDs, knownPeripheralID: knownID),
                    let restored = restoredPeripherals.first(where: { $0.identifier == restoredID }) {
                    peripheral = restored
                    restored.delegate = self
                    weatherUploadStartedAt = weatherRequestClock.now
                    weatherUploadConnectedAt = nil
                    weatherUploadConnectedDeadlineAt = nil
                    armWeatherUploadDeadline(until: uploadDeadline)
                    print("[OBC BLE weather] restored upload intent for known peripheral \(knownID)")
                    // `willRestoreState` runs *before* the first `centralManagerDidUpdateState`, so
                    // the manager is normally still `.unknown` here and a connect issued now is
                    // dropped without a callback — the adopted intent would then sit `.connecting`
                    // until its 90 s deadline with no radio activity at all. Defer to `.poweredOn`.
                    weatherUploadRestoredConnectPending = true
                    resumeRestoredWeatherUploadConnect()
                    return
                }
            }
            discoveryStore.clearWeatherUploadRestoration()
        }
        guard let deadline = discoveryStore.weatherRestorationDeadline(), deadline > Date()
        else {
            discoveryStore.clearWeatherRestoration()
            return
        }
        let restoredServices = (dict[CBCentralManagerRestoredStateScanServicesKey] as? [CBUUID] ?? [])
        let services = Set(restoredServices.compactMap { uuid -> BLEDiscoveryIntentPolicy.Service? in
            if uuid == GATT.obcControlService { return .control }
            if uuid == GATT.weatherRequestService { return .weatherRequest }
            return nil
        })
        let restoredPeripherals = dict[CBCentralManagerRestoredStatePeripheralsKey] as? [CBPeripheral] ?? []
        let restoredIDs = Set(restoredPeripherals.map(\.identifier))
        let restoredID = discoveryPolicy.restoreWeatherRequest(
            scannedServices: services, restoredPeripheralIDs: restoredIDs, knownPeripheralID: knownID
        )
        guard discoveryPolicy.weatherRequestPending else { return }

        let now = weatherRequestClock.now
        weatherRequestStartedAt = now
        weatherRequestDiscoveredAt = restoredID == nil ? nil : now
        weatherRequestConnectedAt = nil
        weatherRequestConnectedDeadlineAt = nil
        weatherRequestReusedForeground = false
        armWeatherRequestDeadline(until: deadline)
        print("[OBC BLE weather] restored weather-only intent for known peripheral \(knownID)")

        guard let restoredID,
              let restored = restoredPeripherals.first(where: { $0.identifier == restoredID })
        else {
            startConnectIfReady()
            return
        }
        peripheral = restored
        restored.delegate = self
        switch restored.state {
        case .connected:
            self.centralManager(central, didConnect: restored)
        case .connecting:
            break // CoreBluetooth will deliver didConnect/didFail on this same manager.
        default:
            central.connect(restored)
        }
    }

    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                               advertisementData: [String: Any], rssi RSSI: NSNumber) {
        let action = discoveryPolicy.discovered(
            peripheralID: peripheral.identifier,
            knownPeripheralID: discoveryStore.knownPeripheralID()
        )
        let owner: BLEDiscoveryIntentPolicy.Ownership
        switch action {
        case .ignore:
            return
        case .connect(let connectionOwner):
            owner = connectionOwner
        case .connectForWeatherRead(let connectionOwner):
            // A standing watch or known-peer foreground recovery needs an autonomous probe: arm
            // its bookkeeping (deadline, restoration intent, timing evidence). Its result reaches
            // the job engine via `weatherRequestEvents`; resting contexts are filtered there.
            armAutonomousWeatherRead()
            owner = connectionOwner
        }
        central.stopScan()
        activeScanServices.removeAll()
        self.peripheral = peripheral
        peripheral.delegate = self
        if owner == .weatherRequest { weatherRequestDiscoveredAt = weatherRequestClock.now }
        central.connect(peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        discoveryPolicy.didConnect(peripheralID: peripheral.identifier)
        if discoveryPolicy.connectionOwnership == .weatherRequest {
            // The radio hold this connection represents starts here, and both weather legs are
            // measured against it — a read that hands the link to the upload does not buy the
            // upload a fresh window (see `armWeatherUploadConnectedDeadline`).
            if weatherOwnedConnectionUpAt == nil { weatherOwnedConnectionUpAt = .now() }
            if discoveryPolicy.weatherRequestPending {
                weatherRequestConnectedAt = weatherRequestClock.now
                armWeatherRequestConnectedDeadline()
            } else if discoveryPolicy.weatherUploadPending {
                weatherUploadConnectedAt = weatherRequestClock.now
                armWeatherUploadConnectedDeadline()
            }
        } else {
            weatherOwnedConnectionUpAt = nil
        }
        armDiscoveryWatchdog()  // bounds GATT discovery, not the scan/reconnect wait (#302)
        let services: [CBUUID]
        if discoveryPolicy.foregroundRequested {
            // Foreground discovery accepts either advertisement and always discovers both custom
            // services, preserving the normal Control session when the weather UUID was on air.
            services = [GATT.deviceInformation, GATT.battery, GATT.obcControlService, GATT.weatherRequestService]
        } else if discoveryPolicy.weatherUploadPending {
            // The upload leg needs objectControl + PSM for the ordinary v4 transfer; the weather
            // service rides along for a read that may share
            // this ephemeral connection.
            services = [GATT.obcControlService, GATT.weatherRequestService]
        } else {
            services = [GATT.weatherRequestService]
        }
        peripheral.discoverServices(services)
    }

    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        weatherOwnedConnectionUpAt = nil
        if discoveryPolicy.connectionOwnership == .weatherRequest {
            if discoveryPolicy.weatherRequestPending { failWeatherRequest(.connectionDropped) }
            if discoveryPolicy.weatherUploadPending { failWeatherUpload(.connectionDropped) }
            discoveryPolicy.didDisconnect()
            return
        }
        if discoverContinuation != nil {
            failDiscover(.notConnected)
        } else if discoveryPolicy.foregroundRequested {
            // Re-enter discovery rather than blindly retrying this cached peripheral. The next
            // advertisement may be Weather Request rather than Control, and its UUID is the only
            // evidence that arms the autonomous context read.
            discoveryPolicy.didDisconnect()
            startConnectIfReady()
        }
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        weatherOwnedConnectionUpAt = nil
        if discoveryPolicy.weatherRequestPending { failWeatherRequest(.connectionDropped) }
        if discoveryPolicy.weatherUploadPending { failWeatherUpload(.connectionDropped) }
        discoveryPolicy.didDisconnect()
        characteristics.removeAll()
        disarmDiscoveryWatchdog()  // the channel watchdog is disarmed by failAllPending below
        // Close the dead CoC, don't just drop the reference: an `L2CAPByteChannel`
        // owns a dedicated run-loop thread + stall `Timer` that only stop via
        // `close()`/`teardown`. Nil-ing the refs alone orphans a thread that keeps
        // waking every 0.25 s — one leaked per disconnect (S4 out-of-range
        // flapping). We're already on `queue`, so drop the refs inline and fire the
        // async close (which also resolves the channel's own parked read/write
        // waiters); the `Task` retains the channel until teardown completes.
        let deadChannel = byteChannel
        byteChannel = nil
        bleChannel = nil
        if let deadChannel { Task { await deadChannel.close() } }
        failAllPending()
        // A disconnect that lands while a connect phase is still pending IS that
        // phase's failure. `failAllPending` deliberately leaves the two phase
        // continuations alone, and a declined / wrong passkey commonly tears the
        // link down instead of erroring the gated PSM read — so without this a
        // fresh pair hangs `confirmPairing()` on the D3 beat forever (there is no
        // timeout on `authenticate()`). Both helpers drop `wantsConnect`, which
        // also stops the reconnect loop from silently re-raising the passkey sheet
        // after a decline.
        if discoverContinuation != nil {
            failDiscover(.notConnected)
            return
        }
        if authenticateContinuation != nil {
            failAuthenticate(.pairingFailed)
            return
        }
        // #753: a drop during the gated-phase retry beat (continuation momentarily
        // nil) is terminal, like a drop during a pending authenticate — the second
        // attempt will fail `.notConnected` onto D5. Drop the intent so the
        // reconnect loop below can't re-raise the passkey behind it.
        if awaitingGatedRetry {
            awaitingGatedRetry = false
            _ = discoveryPolicy.cancelForeground()
            stateMulticast.send(.disconnected)
            return
        }
        stateMulticast.send(discoveryPolicy.foregroundRequested ? .outOfRange : .disconnected)
        // Reconnect through discovery rather than a blind direct connect. This must include the
        // standing weather watch: backgrounding deliberately disconnects the foreground session,
        // and `didDisconnect()` moves that retained watch to `.scanning`; failing to execute the
        // phase left CoreBluetooth with no actual scan and made every later device request silent.
        // `hasIntent` covers foreground recovery, a one-shot weather read, and that standing watch.
        if discoveryPolicy.hasIntent {
            startConnectIfReady()
        }
    }
}

// MARK: - CBPeripheralDelegate

extension BLETransport: CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil else {
            failConnectionSetup()
            return
        }
        let services = peripheral.services ?? []
        guard !services.isEmpty else {
            failConnectionSetup()
            return
        }
        pendingServiceDiscovery = services.count
        for service in services {
            peripheral.discoverCharacteristics(nil, for: service)
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard error == nil else {
            failConnectionSetup()
            return
        }
        for characteristic in service.characteristics ?? [] {
            characteristics[characteristic.uuid] = characteristic
            // Only the **un-gated** BAS notify is armed here (#297). The gated
            // the `objectControl` indication and the PSM read wait for
            // `authenticate()`, so first-time pairing doesn't raise the passkey
            // sheet before the D2 row tap. The device's connect-time battery notify
            // fires before this subscription lands (its next is ~30 s out) — read
            // the level so the UI has it at once; it resolves through the same
            // didUpdateValueFor path as a notify.
            if characteristic.uuid == GATT.batteryLevel {
                peripheral.setNotifyValue(true, for: characteristic)
                peripheral.readValue(for: characteristic)
            }
        }
        pendingServiceDiscovery -= 1
        guard pendingServiceDiscovery <= 0 else { return }
        disarmDiscoveryWatchdog()  // discovery completed — the un-gated surface is ready (#302)
        if discoveryPolicy.weatherRequestPending { beginWeatherRequestReadIfReady() }
        if discoveryPolicy.weatherUploadPending { beginWeatherUploadIfReady() }
        // Every service's characteristics are in hand — the un-gated surface is
        // ready. A pending `discover()` resolves here (its caller runs
        // `authenticate()` next, on the D2 row tap); an unsolicited background
        // reconnect (bonded, no waiter) proceeds straight to the gated phase to
        // restore the full link.
        if discoveryPolicy.foregroundRequested {
            if let cont = discoverContinuation {
                discoverContinuation = nil
                cont.resume()
            } else {
                beginAuthenticate()
            }
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        let uuid = characteristic.uuid

        // BAS battery notify → multicast.
        if uuid == GATT.batteryLevel, let value = characteristic.value?.first {
            batteryMulticast.send(Int(value))
            return
        }
        // PSM read → open the L2CAP channel (initial connect and re-opens alike).
        if uuid == GATT.psm, bleChannel == nil {
            if error == nil, let data = characteristic.value, data.count >= 2 {
                let psm = UInt16(data[0]) | (UInt16(data[1]) << 8)
                // The read resolved, so any passkey entry is behind us — re-arm
                // tight for the machine-only openL2CAPChannel → didOpen tail
                // (#302's actual wedge). Only while this open is still the one
                // being watched; a stale resolve after the watchdog fired
                // (openingChannel already dropped) opens unwatched, as before.
                if openingChannel { armChannelWatchdog(after: Self.phaseTimeout) }
                peripheral.openL2CAPChannel(CBL2CAPPSM(psm))
            } else {
                // The PSM characteristic is `authenticated` (A8): the read is the
                // first gated op of `authenticate()`, so a failure here is usually
                // the pairing being declined / the wrong passkey (ATT insufficient-
                // authentication). Fail the open waiters AND the pending
                // authenticate — else `confirmPairing()` hangs in the D3 beat. An
                // auth-class error → `pairingFailed` (D5 "didn't finish"); anything
                // else → `channelOpenFailed`. (A decline that instead *disconnects*
                // the link lands via `didDisconnectPeripheral`; on-glass polish.)
                openingChannel = false
                disarmChannelWatchdog()
                let waiters = channelWaiters
                channelWaiters.removeAll()
                // #753: an auth-class failure on the gated PSM read while the
                // peripheral is *still connected* is the conservative "pairing
                // visibly completed, firmware momentarily refused" proxy (see
                // `GatedPairingWindowError` — `isRetryableGatedFailure` is shared
                // with the gated CCCD-write branch). Resolve a parked fresh-pair
                // `authenticate()` as retryable — it retries the gated phase once
                // on this bonded link instead of dropping to D5. Any transfer's
                // channel waiters (none during a fresh pair) fail as before; the
                // retry re-opens the CoC.
                if Self.isRetryableGatedFailure(
                    error, peripheralConnected: peripheral.state == .connected,
                    authenticatePending: authenticateContinuation != nil
                ) {
                    for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
                    resolveAuthenticateRetryable()
                } else {
                    let failure: DeviceError = Self.isAuthError(error) ? .pairingFailed : .channelOpenFailed
                    for cont in waiters { cont.resume(throwing: failure) }
                    if authenticateContinuation != nil { failAuthenticate(failure) }
                }
            }
            return
        }
        if uuid == GATT.objectControl {
            guard error == nil, let record = characteristic.value else {
                let waiters = objectControlWaiters
                objectControlWaiters.removeAll()
                for waiter in waiters { waiter.resume(throwing: TransferLinkLost()) }
                return
            }
            if objectControlWaiters.isEmpty {
                pendingObjectControlRecords.append(record)
            } else {
                objectControlWaiters.removeFirst().resume(returning: record)
            }
            return
        }

        // Resolve a pending read.
        resumeReads(uuid, error == nil ? .success(characteristic.value ?? Data()) : .failure(DeviceError.readFailed))
    }

    public func peripheral(
        _ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?
    ) {
        // #753: on a fresh pair the FIRST gated op is a CCCD write, not the PSM
        // read — `beginAuthenticate` arms the `objectControl` indication before reading the
        // PSM — so it's the CCCD write that raises the passkey sheet, and iOS's
        // post-passkey replay of the gated ops hits the CCCD write *first*. The
        // firmware's post-PairingComplete refusal window therefore most likely
        // clips the CCCD write; without this handler that failure was silently
        // swallowed — and if the window then drained before the PSM read,
        // `authenticate()` resolved with a dead control indication. Map it exactly like the PSM branch: the
        // shared retryable proxy → resolve the parked authenticate as retryable,
        // and the retry's `beginAuthenticate` re-arms the gated ops. Everything
        // else keeps the pre-existing ignore: a background re-arm has no
        // authenticate pending, and a real decline tears the link down
        // (`didDisconnectPeripheral` owns that path). No notify-state bookkeeping
        // beyond this. `objectControl` is the sole gated CCCD this window can clip.
        guard characteristic.uuid == GATT.objectControl else { return }
        guard Self.isRetryableGatedFailure(
            error, peripheralConnected: peripheral.state == .connected,
            authenticatePending: authenticateContinuation != nil
        ) else { return }
        resolveAuthenticateRetryable()
    }

    public func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let result: Result<Void, Error> = error == nil ? .success(()) : .failure(DeviceError.writeFailed)
        let conts = pendingWrites.removeValue(forKey: characteristic.uuid) ?? []
        for cont in conts { cont.resume(with: result) }
    }

    public func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        openingChannel = false
        disarmChannelWatchdog()
        guard let channel, error == nil else {
            let waiters = channelWaiters
            channelWaiters.removeAll()
            for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
            if authenticateContinuation != nil { failAuthenticate(.channelOpenFailed) }
            return
        }
        let byte = L2CAPByteChannel(channel: channel)
        byteChannel = byte
        let ble = BLEChannel(channel: byte)
        bleChannel = ble
        let waiters = channelWaiters
        channelWaiters.removeAll()
        for cont in waiters { cont.resume(returning: ble) }
        // CoC up + services discovered → the link is ready. `finishConnect`
        // publishes .connected either way and resolves `authenticate()`'s
        // continuation when one is pending — a background *re*connect (after a
        // drop) has none, but must still flip the state stream back.
        finishConnect()
    }

    private func resumeReads(_ uuid: CBUUID, _ result: Result<Data, Error>) {
        let conts = pendingReads.removeValue(forKey: uuid) ?? []
        for cont in conts { cont.resume(with: result) }
    }
}
#endif
