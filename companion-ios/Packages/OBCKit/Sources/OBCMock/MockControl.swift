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
/// contract's `DeviceError` doesn't model pairing UX). `.timeout` fails in the
/// `discover()` phase (`radioGate`), `.rejected` in `authenticate()` (`pairingGate`),
/// mapped onto the closest `DeviceError`; the UI keys the exact copy off `scenario`.
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
    /// `nil` seed = no replay, matching the real transport: a `storeChanged` is
    /// an edge, not a state (see `DeviceTransport.storeChanges`).
    let storeChangedMulticast = AsyncMulticast<StoreChanged?>(nil)

    private let lock = NSLock()
    private var _scenario: Scenario
    private var _latency: Duration
    private var _throughput: Int
    private var _radio: RadioState
    private var _bonded: Bool
    private var _bondedName: String?
    private var _pairingFail: PairingFail?
    private var _pendingFailures: [DeviceError]
    private var _dropFraction: Double?
    private var _tripListFailure: DeviceError?
    private var _fixtures: FixtureSet
    /// Monotonic stand-in for the device's object-id assignment on upload — starts
    /// above every fixture `deviceObjectID` so a fresh id can't collide.
    private var _nextObjectID: UInt16 = 1000
    /// The device's **trip** object store (TR8) — trips uploaded this session,
    /// keyed by the id the device assigned. Empty at boot (a trip lands on the
    /// device only via a whole-trip upload); `listTrips()` serves its catalog.
    private var _deviceTrips: [DeviceTrip] = []
    /// The device's trip-id counter — its own namespace, distinct from route ids
    /// (spec §4.1). Any base works; the app never assumes a value.
    private var _nextTripID: UInt16 = 1
    /// Device object ids the app asked the device to delete this session, in
    /// order — the "Delete trip & routes" cascade tests assert the per-route +
    /// trip delete commands landed (routes then trip, or however composed).
    private var _deletedRouteObjectIDs: [DeviceObjectID] = []
    private var _deletedTripObjectIDs: [DeviceObjectID] = []
    /// When set, `deviceRoutes()` pads its catalog to one below the route cap so
    /// a multi-stage fresh trip fails the whole-trip precheck **before any bytes**
    /// (issue #657) — the storage-precheck XCUITest hook (`-OBCDeviceRoutesFull`).
    private var _routesNearlyFull = false
    /// Every `ackRides` batch the transport sent, in order — the coordinator
    /// tests assert the connect-time possession ack lands here.
    private var _ackedRideBatches: [[RideID]] = []
    /// How many `forgetBond` commands (#756) the transport sent — the forget
    /// tests assert a connected forget reaches the device before clearing.
    private var _forgetBondCount = 0
    /// Test-only evidence that a catalog read was abandoned by its caller. A
    /// store-change burst must coalesce behind a live read instead of cancelling
    /// it halfway through the real transport's CoC exchange.
    private var _cancelledRouteListReadCount = 0
    /// The version string of the firmware the phone staged this session (from the
    /// last completed `fwImage` upload) — what a modelled reboot reconnects on.
    /// `nil` until an upload completes.
    private var _firmwareStagedVersion: String?
    /// What the next `installFw` request answers (S7). Default `accepted` — the
    /// happy path, after which the modelled device reboots onto the staged version.
    private var _firmwareInstallOutcome: FirmwareInstallResult = .accepted

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

    /// The bond record's saved device name — the *desired* name after a rename,
    /// which diverges from `deviceInfo` when the config write failed (#361;
    /// the reconcile pass keys off exactly that gap). `nil` until a save;
    /// `MockBondStore` then serves `deviceInfo.name` (scenario boots).
    public var bondedName: String? {
        get { lock.withLocked { _bondedName } }
        set { lock.withLocked { _bondedName = newValue } }
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
            _bondedName = nil
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

    /// Fail the **next** `listTrips()` read (one-shot; a retry succeeds) — the
    /// transient trip-catalog failure a reload's reconcile must survive without
    /// reading it as "the device stores zero trips" (which dropped every trip
    /// link and made the next whole-trip upload mint a duplicate device trip).
    /// Targeted (unlike `failNextOp`) so a test can fail `listTrips` while the
    /// same reload's `listRoutes` succeeds — the exact on-glass sequence.
    public func failNextTripList(_ error: DeviceError = .readFailed) {
        lock.withLocked { _tripListFailure = error }
    }

    /// Consume the armed `listTrips` failure, if any (one-shot).
    func takeTripListFailure() throws {
        let error: DeviceError? = lock.withLocked {
            defer { _tripListFailure = nil }
            return _tripListFailure
        }
        if let error { throw error }
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

    // MARK: Library seeding (composition root + tests)

    /// Write the fixture routes into `store` as library records (B1S) — the
    /// Planned list is library-first (#289), so a scenario's routes exist as
    /// phone-side saves, with `deviceObjectID` marking the ones the mock device
    /// also holds. Descending `addedAt` keeps the fixture order in the list.
    /// Idempotent: ids already in the store are left untouched (a "relaunch"
    /// over the same store must not reshuffle what the user saved since).
    public func seedLibrary(into store: any LibraryStore) {
        let existing = Set(store.plannedRoutes().map(\.id))
        let routes = lock.withLocked { _fixtures.routes }
        // Seeded device links carry the mock device's own (serial, epoch)
        // scope (#769) — the same link an upload against this mock would mint,
        // so badges and replace-by-id behave exactly as on the real path.
        let scope = deviceInfo.libraryScope
        let base = Date()
        for (index, entry) in routes.enumerated() where !existing.contains(entry.summary.id) {
            var record = entry.record(addedAt: base.addingTimeInterval(-Double(index)), scope: scope)
            // A fixture the mock device holds boots **up to date** — the seeded
            // fingerprint matches what an upload of the record would send, so
            // the C1 badge shows the check (a rename then flips it to outdated,
            // same as on the real path).
            if record.deviceLink != nil {
                record.uploadedCRC32 = RouteObjectCodec.payloadCRC(for: record)
            }
            store.savePlannedRoute(record)
        }
        // TR6: seed trips grouping some of those routes (a trip is phone-side
        // metadata; the routes are written above first so no stage dangles).
        // Idempotent over trip ids, like the routes above.
        let trips = lock.withLocked { _fixtures.trips }
        let existingTrips = Set(store.trips().map(\.id))
        for entry in trips where !existingTrips.contains(entry.id) {
            store.saveTrip(entry.record(base: base))
        }
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

    /// Radio + scan gate for the un-gated `discover()` phase (#297): H7/H8, plus a
    /// `.timeout` pairing fault — the device never turns up in the scan window, so
    /// it surfaces here (before the D2 row), not at the row tap.
    func radioGate() throws {
        let (radio, pairing) = lock.withLocked { (_radio, _pairingFail) }
        switch radio {
        case .on: break
        case .off: throw DeviceError.bluetoothUnavailable(.poweredOff)
        case .unauthorized: throw DeviceError.bluetoothUnavailable(.unauthorized)
        }
        if pairing == .timeout { throw DeviceError.deviceNotFound }
    }

    /// Pairing gate for the gated `authenticate()` phase (#297): a declined / wrong
    /// passkey (D5 rejected) — what the D2 row tap now triggers, mirroring the real
    /// path's LESC sheet. Maps onto `pairingFailed`; the UI keys the copy off
    /// `scenario`.
    func pairingGate() throws {
        if lock.withLocked({ _pairingFail }) == .rejected { throw DeviceError.pairingFailed }
    }

    /// Update the on-device config; renaming (Delta 1) also updates the reported name.
    func setConfig(_ config: DeviceConfig) {
        lock.withLocked {
            _fixtures.config = config
            _fixtures.deviceInfo = _fixtures.deviceInfo.renamed(config.name)
        }
    }

    /// The device's route catalog — the fixture routes it holds a copy of
    /// (`deviceObjectID != nil`), keyed by their device object ids, exactly the
    /// shape the real `routeList` download produces. The app consumes this only
    /// to reconcile the "on device" badge (#289) — never as list rows.
    func deviceRoutes() -> [RouteCatalogEntry] {
        var catalog = lock.withLocked { _fixtures.routes }.compactMap { entry -> RouteCatalogEntry? in
            guard let objectID = entry.deviceObjectID else { return nil }
            // The v2 `routeList` carries the whole-object CRC (#770): a real
            // upload pinned it (`recordDeviceCopy`); a seeded copy derives it
            // from the fixture geometry — the same OBCR encoding `seedLibrary`
            // fingerprints, so a device-held fixture boots proven up to date.
            let crc32 = entry.crc32 ?? RouteObjectCodec.payloadCRC(for: entry.record(addedAt: Date()))
            return RouteCatalogEntry(
                id: objectID, name: entry.summary.name,
                distanceMeters: entry.summary.distanceMeters,
                elevationGainMeters: entry.summary.elevationGainMeters,
                pointCount: entry.summary.pointCount,
                crc32: crc32
            )
        }
        // Storage-precheck hook: pad to one below the route cap so exactly one
        // slot is free — a multi-stage fresh trip then can't fit (issue #657).
        if lock.withLocked({ _routesNearlyFull }) {
            let used = Set(catalog.map(\.id.raw))
            var filler = UInt16(50_000)
            while catalog.count < DeviceStorage.routeCapacity - 1 {
                while used.contains(filler) { filler &+= 1 }
                catalog.append(RouteCatalogEntry(
                    id: DeviceObjectID(filler), name: "On-device route \(filler)",
                    distanceMeters: 0, elevationGainMeters: 0, pointCount: 0, crc32: 0))
                filler &+= 1
            }
        }
        return catalog
    }

    /// The stored copy behind a device object id (`routeDetail` on the mock).
    func deviceRouteEntry(_ id: DeviceObjectID) -> RouteEntry? {
        lock.withLocked { _fixtures.routes.first { $0.deviceObjectID == id } }
    }

    /// Record an `ackRides` possession batch (the mock's stand-in for the
    /// device's sidecar reconcile — the mock models no device-side synced
    /// state, so recording is the observable effect).
    func recordAckedRides(_ ids: [RideID]) {
        lock.withLocked { _ackedRideBatches.append(ids) }
    }

    /// The `ackRides` batches sent so far, in send order (test hook).
    public var ackedRideBatches: [[RideID]] {
        lock.withLocked { _ackedRideBatches }
    }

    /// Record a `forgetBond` request (#756, the mock's stand-in for the device
    /// dissolving its side of the bond). The mock models no device-side bond
    /// slot, so the count is the observable effect the forget tests assert.
    func recordForgetBond() {
        lock.withLocked { _forgetBondCount += 1 }
    }

    /// How many `forgetBond` commands the app has sent (test hook). Non-zero means
    /// the connected forget reached the device before clearing the local record.
    public var forgetBondCount: Int {
        lock.withLocked { _forgetBondCount }
    }

    public var cancelledRouteListReadCount: Int {
        lock.withLocked { _cancelledRouteListReadCount }
    }

    func recordCancelledRouteListReadIfNeeded() {
        if Task.isCancelled { lock.withLocked { _cancelledRouteListReadCount += 1 } }
    }

    /// Delete a stored route by its device object id (the `deleteObject`
    /// command): the device forgets its copy — the library record (and its list
    /// row) is the app's own business.
    func removeRoute(_ id: DeviceObjectID) {
        lock.withLocked {
            for index in _fixtures.routes.indices where _fixtures.routes[index].deviceObjectID == id {
                _fixtures.routes[index].deviceObjectID = nil
            }
        }
    }

    /// Simulate an **on-device** route delete (epic #447 P6, the Route menu's
    /// hold-to-delete): the device forgets its copy and notifies `storeChanged`,
    /// exactly the wire sequence the real firmware sends — the app's live
    /// badge-reconcile input. Dev-panel/test hook.
    public func deviceDeletesRoute(_ id: DeviceObjectID) {
        removeRoute(id)
        storeChangedMulticast.send(StoreChanged(type: .route, revision: 0))
    }

    // MARK: Trips (TR8 — the device-side trip object store)

    /// Whether a device holds a copy of `routesNearlyFull` — the storage-precheck
    /// XCUITest hook. When true, `deviceRoutes()` pads its catalog so exactly one
    /// route slot is free, forcing a multi-stage fresh trip to fail the precheck.
    public var routesNearlyFull: Bool {
        get { lock.withLocked { _routesNearlyFull } }
        set { lock.withLocked { _routesNearlyFull = newValue } }
    }

    /// One trip the (mock) device stores — the device's own copy, keyed by the id
    /// it assigned. `stageIDs` are **device** route object ids in ride order.
    struct DeviceTrip: Sendable {
        var id: DeviceObjectID
        var name: String
        var stageIDs: [DeviceObjectID]
        var payloadByteCount: Int
        var crc32: UInt32
    }

    /// The device's trip catalog (`tripList`, spec §7.4) — one entry per stored
    /// trip, its stats **summed over resolvable stages** (a stage whose device
    /// route is gone is dangling: counted in `stageCount`, excluded from the
    /// totals), exactly as the firmware computes them. Reconcile input only.
    func deviceTripCatalog() -> [TripCatalogEntry] {
        lock.withLocked {
            _deviceTrips.map { trip in
                var distance = 0.0
                var ascent = 0.0
                for stageID in trip.stageIDs {
                    guard let route = _fixtures.routes.first(where: { $0.deviceObjectID == stageID }) else { continue }
                    distance += route.summary.distanceMeters
                    ascent += route.summary.elevationGainMeters
                }
                return TripCatalogEntry(
                    id: trip.id, name: trip.name,
                    distanceMeters: distance, elevationGainMeters: ascent,
                    stageCount: trip.stageIDs.count, crc32: trip.crc32
                )
            }
        }
    }

    /// The stored trip object behind a device id (`downloadTrip` on the mock) —
    /// byte-faithful decode of what an upload wrote (dangling refs included).
    func deviceTripDecoded(_ id: DeviceObjectID) -> TripObjectCodec.Decoded? {
        guard let trip = (lock.withLocked { _deviceTrips.first { $0.id == id } }) else { return nil }
        return TripObjectCodec.Decoded(version: TripObjectCodec.version, name: trip.name, stageObjectIDs: trip.stageIDs)
    }

    /// Begin a simulated trip upload (TR8). New trips (`targetObjectID == nil`)
    /// take a fresh id from the trip counter; a `storageFull` reject fires at
    /// descriptor-open when the trip catalog is at its cap (replace-by-id is
    /// exempt, spec §7.4). On commit the device records the copy so a later
    /// `listTrips()` reconcile keeps the badge lit.
    func beginTripUpload(_ blob: TripBlob) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        if blob.payload.isEmpty { return .immediatelyFinished(.failed(.transferRejected)) }
        let isNew = blob.targetObjectID == nil
        // Fresh-upload dedup (spec §4.2), mirroring the device's commit-time rule:
        // identical content re-sent as a *new* object (a retry after a lost commit
        // ack) converges on the stored copy — the transfer paces and completes as
        // normal, but the assigned id is the existing trip's and nothing new is
        // stored. Checked before the cap (a dedup hit consumes no slot).
        let dedupID: DeviceObjectID? = !isNew ? nil : lock.withLocked {
            let crc = CRC32.checksum(blob.payload)
            return _deviceTrips.first { $0.crc32 == crc }?.id
        }
        // Cap check at open (new only): a full trip catalog rejects with storageFull.
        let full = lock.withLocked { _deviceTrips.count >= DeviceStorage.tripCapacity }
        if isNew && full && dedupID == nil { return .immediatelyFinished(.failed(.storageFull)) }
        let assignedID = AsyncPromise<DeviceObjectID?>()
        // A trip object is tiny; pace off a small design-scale minimum so the F
        // bar is still visible (the mock's realism is timing, not wire bytes).
        let pacingBytes = max(blob.payload.count, 4_000, 1)
        let handle = startTransfer(total: pacingBytes, segments: [], rides: nil, assignedObjectID: assignedID)
        let objectID = blob.targetObjectID ?? dedupID ?? lock.withLocked { () -> DeviceObjectID in
            let id = _nextTripID
            _nextTripID &+= 1
            return DeviceObjectID(id)
        }
        Task { [weak self] in
            guard await handle.outcome == .completed else {
                assignedID.fulfill(nil)
                return
            }
            self?.recordDeviceTripCopy(of: blob, objectID: objectID)
            assignedID.fulfill(objectID)
        }
        return handle
    }

    /// A committed trip upload landed on the (mock) device: store (or replace) the
    /// copy under `objectID`, CRCing exactly the payload bytes received (#770's
    /// rule for routes, applied to trips) so a re-list proves the badge against
    /// the fingerprint the app committed.
    private func recordDeviceTripCopy(of blob: TripBlob, objectID: DeviceObjectID) {
        let committedCRC = CRC32.checksum(blob.payload)
        let stored = DeviceTrip(
            id: objectID, name: blob.name, stageIDs: blob.deviceStageIDs,
            payloadByteCount: max(1, blob.payload.count), crc32: committedCRC)
        lock.withLocked {
            if let index = _deviceTrips.firstIndex(where: { $0.id == objectID }) {
                _deviceTrips[index] = stored
            } else {
                _deviceTrips.append(stored)
            }
        }
    }

    /// Delete a stored trip by device id (`deleteObject` for a trip) — **non
    /// cascading**: the trip metadata goes, member device routes stay. Records the
    /// id so the cascade tests can assert the command reached the device.
    func removeTrip(_ id: DeviceObjectID) {
        lock.withLocked {
            _deviceTrips.removeAll { $0.id == id }
            _deletedTripObjectIDs.append(id)
        }
    }

    /// Record an app-issued route delete (the "Delete trip & routes" cascade's
    /// per-route half) alongside the store mutation — so a test asserts both
    /// commands landed. `removeRoute` handles the store side.
    func recordRouteObjectDelete(_ id: DeviceObjectID) {
        lock.withLocked { _deletedRouteObjectIDs.append(id) }
    }

    /// The device object ids the app deleted this session (test hooks).
    public var deletedRouteObjectIDs: [DeviceObjectID] { lock.withLocked { _deletedRouteObjectIDs } }
    public var deletedTripObjectIDs: [DeviceObjectID] { lock.withLocked { _deletedTripObjectIDs } }
    /// How many trips the (mock) device currently stores (test hook).
    public var deviceTripCount: Int { lock.withLocked { _deviceTrips.count } }
    /// The device object ids of the trips currently stored (test hook) — lets a
    /// test drive a device-side trip delete against a real assigned id.
    public var deviceTripObjectIDs: [DeviceObjectID] { lock.withLocked { _deviceTrips.map(\.id) } }
    /// The stage device ids the device trip `id` references (test hook).
    public func deviceTripStageIDs(_ id: DeviceObjectID) -> [DeviceObjectID] {
        lock.withLocked { _deviceTrips.first { $0.id == id }?.stageIDs ?? [] }
    }

    /// Simulate an **on-device trip delete** (TR3 long-press → cascade): the
    /// device forgets the trip **and its member routes** and notifies
    /// `storeChanged` for both stores — the wire sequence the real firmware sends,
    /// the app's live badge-reconcile input. Dev-panel/test hook.
    public func deviceDeletesTripCascade(_ id: DeviceObjectID) {
        let stageIDs: [DeviceObjectID] = lock.withLocked {
            let stages = _deviceTrips.first { $0.id == id }?.stageIDs ?? []
            _deviceTrips.removeAll { $0.id == id }
            return stages
        }
        for stageID in stageIDs { removeRoute(stageID) }
        storeChangedMulticast.send(StoreChanged(type: .route, revision: 0))
        storeChangedMulticast.send(StoreChanged(type: .trip, revision: 0))
    }

    /// Simulate a device-side **trip-only** delete (no cascade) — clears the
    /// app's trip link at reconcile while the member routes stay.
    public func deviceDeletesTrip(_ id: DeviceObjectID) {
        lock.withLocked { _deviceTrips.removeAll { $0.id == id } }
        storeChangedMulticast.send(StoreChanged(type: .trip, revision: 0))
    }

    /// Begin a simulated route upload. On commit it reports a device object id (a
    /// fresh monotonic id, or the `targetObjectID` when replacing) so the app can
    /// record the route as on-device — and the fixture set records the copy, so a
    /// later `listRoutes()` reconcile keeps the badge lit. Paced over a
    /// **design-scale fiction** (≈37 B/m), decoupled from the real OBCR payload —
    /// a real route is only a few kB and its F screen would flash by; the mock's
    /// realism is timing + faults, not wire bytes (the same reason ride downloads
    /// pace off `downloadByteCount`).
    func beginRouteUpload(_ blob: RouteBlob) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        if blob.payload.isEmpty { return .immediatelyFinished(.failed(.transferRejected)) }
        // Fresh-upload dedup (spec §4.2) — see `beginTripUpload`: a re-sent new
        // object whose payload CRC matches an on-device copy answers with that
        // copy's id instead of minting a twin. Entries with no pinned CRC (a
        // seeded fixture never uploaded) never match — the device's posture for
        // a side-loaded file with an unfilled sidecar.
        let dedupID: DeviceObjectID? = blob.targetObjectID != nil ? nil : lock.withLocked {
            let crc = CRC32.checksum(blob.payload)
            return _fixtures.routes.first { $0.deviceObjectID != nil && $0.crc32 == crc }?.deviceObjectID
        }
        let assignedID = AsyncPromise<DeviceObjectID?>()
        // Whichever is larger: an explicit test payload, or the design-scale
        // minimum from the route length (so a real few-kB OBCR still paces long
        // enough to see F).
        let pacingBytes = max(blob.payload.count, Int(blob.summary.distanceMeters * 37), 1)
        let handle = startTransfer(total: pacingBytes, segments: [], rides: nil, assignedObjectID: assignedID)
        let objectID = blob.targetObjectID ?? dedupID ?? lock.withLocked { () -> DeviceObjectID in
            let id = _nextObjectID
            _nextObjectID &+= 1
            return DeviceObjectID(id)
        }
        Task { [weak self] in
            guard await handle.outcome == .completed else {
                assignedID.fulfill(nil)
                return
            }
            self?.recordDeviceCopy(of: blob, objectID: objectID)
            assignedID.fulfill(objectID)
        }
        return handle
    }

    // MARK: Firmware update (S7)

    /// What the next `installFw` request answers. Set from the debug panel / a
    /// scenario to model `noStaged` / `busy` / etc.
    public var firmwareInstallOutcome: FirmwareInstallResult {
        get { lock.withLocked { _firmwareInstallOutcome } }
        set { lock.withLocked { _firmwareInstallOutcome = newValue } }
    }

    /// Pace a `fwImage` upload (spec §7.6) like a route push. On completion,
    /// remember the container's version so a modelled `installFw` reboot can
    /// reconnect the device onto it.
    func beginFirmwareUpload(_ container: Data) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        if container.isEmpty { return .immediatelyFinished(.failed(.transferRejected)) }
        let version = (try? StagedFirmware.validate(container))?.version
        // Pace off the container, but never so briefly that the F bar can't be
        // seen — a real image is ~850 KB, so a small fixture still feels like one.
        let pacingBytes = max(container.count, 850_000)
        let handle = startTransfer(total: pacingBytes, segments: [], rides: nil)
        Task { [weak self] in
            guard await handle.outcome == .completed else { return }
            self?.lock.withLocked { self?._firmwareStagedVersion = version }
        }
        return handle
    }

    /// Answer an `installFw` request with the configured outcome. On `accepted`,
    /// model the device's reboot: after a beat the link drops and comes back on
    /// the staged version — the update view model's "done" detection.
    func installFirmware() -> FirmwareInstallResult {
        let outcome = lock.withLocked { _firmwareInstallOutcome }
        if outcome == .accepted { scheduleFirmwareReboot() }
        return outcome
    }

    /// Model the post-confirm reboot: drop the link, then reconnect reporting the
    /// staged firmware version (DIS 0x2A26 reflects the newly-installed image).
    /// A no-op if nothing was staged this session.
    private func scheduleFirmwareReboot() {
        let version = lock.withLocked { _firmwareStagedVersion }
        guard let version else { return }
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(2.5))
            guard let self else { return }
            connection = .outOfRange // the device reboots into the bootloader
            try? await Task.sleep(for: .seconds(2.5))
            let current = deviceInfo
            deviceInfo = DeviceInfo(
                name: current.name, firmwareVersion: version,
                hardwareVersion: current.hardwareVersion, serial: current.serial,
                protocolVersion: current.protocolVersion,
                // A DFU install is NOT an era event (RRAM survives): the
                // epoch rides through, like on the real device.
                storeEpoch: current.storeEpoch
            )
            connection = .connecting
            try? await Task.sleep(for: .seconds(1))
            connection = .connected
        }
    }

    /// A committed upload landed on the (mock) device: remember the copy in the
    /// fixture set — replacing the entry that already owns `objectID`, or the
    /// library twin of the same route, before appending a new device-only entry.
    private func recordDeviceCopy(of blob: RouteBlob, objectID: DeviceObjectID) {
        // The device CRCs exactly the payload bytes it received (#770) — the
        // same value the upload sheet reports back to `markRouteUploaded`, so a
        // re-list proves the badge against the fingerprint we committed.
        let committedCRC = CRC32.checksum(blob.payload)
        lock.withLocked {
            if let index = _fixtures.routes.firstIndex(where: {
                $0.deviceObjectID == objectID || $0.summary.id == blob.summary.id
            }) {
                _fixtures.routes[index].summary = blob.summary
                _fixtures.routes[index].waypoints = blob.waypoints
                _fixtures.routes[index].deviceObjectID = objectID
                _fixtures.routes[index].crc32 = committedCRC
            } else {
                _fixtures.routes.append(RouteEntry(
                    summary: blob.summary, waypoints: blob.waypoints,
                    payloadByteCount: max(1, blob.payload.count), deviceObjectID: objectID,
                    crc32: committedCRC
                ))
            }
        }
    }

    /// Begin a simulated ride download: one paced batch whose fixture rides land
    /// (payload = the codec-encoded ride, so the consumer's decode is the real
    /// one) as their bytes complete. Link down (H4) or nothing to pull (H9 up to
    /// date) → both streams already finished.
    func beginRideDownload(_ ids: [RideID]) -> RideDownload {
        let wanted = Set(ids)
        let segments = lock.withLocked {
            _fixtures.rides.filter { wanted.contains($0.summary.id) }
        }.map {
            MockTransfer.Segment(id: $0.summary.id, byteCount: max(1, $0.downloadByteCount),
                                 payload: RideObjectCodec.encode($0.ride()))
        }
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
        rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation?,
        assignedObjectID: AsyncPromise<DeviceObjectID?>? = nil
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
            assignedObjectID: assignedObjectID,
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
                   serial: serial, protocolVersion: protocolVersion, storeEpoch: storeEpoch)
    }
}
#endif
