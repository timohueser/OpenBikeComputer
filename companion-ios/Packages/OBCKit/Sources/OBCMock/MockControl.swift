#if DEBUG
import Foundation
import OBCDomain
import OBCTransport

/// Bluetooth radio power and permission, the actionable subset of `CBManagerState`.
public enum RadioState: Sendable, Equatable {
    case on
    case off
    case unauthorized
}

/// How a pairing attempt fails. Mock-local, because the wire contract's `DeviceError` does not
/// model pairing UX. `.timeout` fails in `discover()`, `.rejected` in `authenticate()`.
public enum PairingFail: Sendable, Equatable {
    case timeout
    case rejected
}

/// A mid-session push the device or the radio can originate. `emit(_:)` routes each onto the live
/// streams or the fixture set; `rideAdded` mutates the enumerable set, because `DeviceObjects`
/// has no rides stream.
public enum DeviceEvent: Sendable {
    case connected
    case disconnected
    case outOfRange
    case batteryChanged(Int)
    case rideAdded(RideSummary)
}

/// The single live fault-injection surface shared by the debug panel, the tests and
/// `MockTransport`. A reference type on purpose: mutate it anywhere and every reader, including
/// every open `state` and `battery` stream, sees the change at once.
/// `@unchecked Sendable` with an `NSLock` around the plain knobs; the two live streams carry
/// their own locks.
public final class MockControl: @unchecked Sendable {
    // Live streams — thread-safe on their own; `connection`/`battery` are views onto them.
    let stateMulticast: AsyncMulticast<ConnectionState>
    let batteryMulticast: AsyncMulticast<Int>
    /// `nil` seed = no replay: a local catalog invalidation is an edge, not state.
    let catalogChangedMulticast = AsyncMulticast<CatalogChange?>(nil)

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
    private var _tripCatalogFailure: DeviceError?
    private var _fixtures: FixtureSet
    /// Starts above every fixture `deviceObjectID`, so a freshly assigned id cannot collide.
    private var _nextObjectID: UInt64 = 1000
    /// Trips uploaded this session, keyed by the id the device assigned. Empty at boot.
    private var _deviceTrips: [DeviceTrip] = []
    /// The trip-id counter, its own namespace. Any base works; the app never assumes a value.
    private var _nextTripID: UInt64 = 1
    /// Device object ids the app asked the device to delete this session, in order.
    private var _deletedRouteObjectIDs: [DeviceObjectID] = []
    private var _deletedTripObjectIDs: [DeviceObjectID] = []
    /// When set, `deviceRoutes()` pads its catalog to the 64-route resident-menu boundary.
    private var _routesNearlyFull = false
    /// How many `forgetBond` commands the transport sent.
    private var _forgetBondCount = 0

    /// Evidence that a catalog read was abandoned by its caller. A store-change burst must
    /// coalesce behind a live read instead of cancelling it halfway.
    private var _cancelledRouteCatalogReadCount = 0
    /// The firmware version the phone staged this session, which a modelled reboot reconnects on.
    private var _firmwareStagedVersion: String?
    /// What the next `installFw` request answers.
    private var _firmwareInstallOutcome: FirmwareInstallResult = .accepted
    private var _supportsClockSync: Bool
    /// Every `setClock` sample the transport sent, in order.
    private var _setClockSamples: [WallClockSample] = []

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
        self._supportsClockSync = preset.supportsClockSync
        self._fixtures = fixtures
    }

    /// Start from `happyPath` but override the reported device identity.
    public convenience init(deviceInfo: DeviceInfo) {
        self.init(scenario: .happyPath)
        self.deviceInfo = deviceInfo
    }

    // MARK: Live knobs

    /// Setting this re-applies the whole preset: fixtures and knobs.
    public var scenario: Scenario {
        get { lock.withLocked { _scenario } }
        set { apply(newValue) }
    }

    /// A view onto the live `state` stream. Setting it pushes to every subscriber.
    public var connection: ConnectionState {
        get { stateMulticast.value }
        set { stateMulticast.send(newValue) }
    }

    /// A view onto the live `battery` stream.
    public var battery: Int {
        get { batteryMulticast.value }
        set { batteryMulticast.send(newValue) }
    }

    /// Per-op delay: the feel of a slow link.
    public var latency: Duration {
        get { lock.withLocked { _latency } }
        set { lock.withLocked { _latency = newValue } }
    }

    public var throughputBytesPerSec: Int {
        get { lock.withLocked { _throughput } }
        set { lock.withLocked { _throughput = max(1, newValue) } }
    }

    /// Gates `connect()`.
    public var radio: RadioState {
        get { lock.withLocked { _radio } }
        set { lock.withLocked { _radio = newValue } }
    }

    /// Whether the app has bonded before, which is what `MockBondStore` serves. Flip it live to
    /// replay first-run pairing.
    public var bonded: Bool {
        get { lock.withLocked { _bonded } }
        set { lock.withLocked { _bonded = newValue } }
    }

    /// The bond record's saved device name: the desired name after a rename, which diverges from
    /// `deviceInfo` when the config write failed. The reconcile pass keys off exactly that gap.
    public var bondedName: String? {
        get { lock.withLocked { _bondedName } }
        set { lock.withLocked { _bondedName = newValue } }
    }

    public var deviceInfo: DeviceInfo {
        get { lock.withLocked { _fixtures.deviceInfo } }
        set { lock.withLocked { _fixtures.deviceInfo = newValue } }
    }

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
            _supportsClockSync = preset.supportsClockSync
            _setClockSamples = []
            _fixtures = fixtures
        }
        stateMulticast.send(preset.connection)
        batteryMulticast.send(fixtures.battery)
    }

    /// Swap the fixture set by bundled-JSON name, leaving the other knobs alone.
    public func loadFixtures(_ named: String) {
        let fixtures = FixtureSet.load(named)
        lock.withLocked { _fixtures = fixtures }
        batteryMulticast.send(fixtures.battery)
    }

    // MARK: Fault injection

    /// Fail the next throwing op with this error. One-shot; a retry succeeds.
    public func failNextOp(_ error: DeviceError) {
        lock.withLocked { _pendingFailures.append(error) }
    }

    /// Arm the next transfer to drop at `fraction` of its bytes. The dropped transfer stalls with
    /// its stream open, and `TransferHandle.resume()` restores the link and finishes it.
    public func dropTransfer(atFraction fraction: Double) {
        lock.withLocked { _dropFraction = min(max(0, fraction), 1) }
    }

    /// Fail the next `listTrips()` read, one-shot. Targeted, unlike `failNextOp`, so a test can
    /// fail `listTrips` while the same reload's `listRoutes` succeeds.
    public func failNextTripCatalog(_ error: DeviceError = .readFailed) {
        lock.withLocked { _tripCatalogFailure = error }
    }

    func takeTripCatalogFailure() throws {
        let error: DeviceError? = lock.withLocked {
            defer { _tripCatalogFailure = nil }
            return _tripCatalogFailure
        }
        if let error { throw error }
    }

    public func setRadio(_ state: RadioState) { radio = state }

    /// Arm the next `connect()` to fail pairing.
    public func failPairing(_ mode: PairingFail) {
        lock.withLocked { _pairingFail = mode }
    }

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

    // MARK: Library seeding

    /// Write the fixture routes into `store` as library records: the Planned list is library-first,
    /// so a scenario's routes exist as phone-side saves, with `deviceObjectID` marking the ones
    /// the mock device also holds. Idempotent, so a relaunch over the same store does not
    /// reshuffle what the user saved since.
    public func seedLibrary(into store: any LibraryStore) {
        let existing = Set(store.plannedRoutes().map(\.id))
        let routes = lock.withLocked { _fixtures.routes }
        // Seeded device links carry the mock device's own serial and StoreId, the same link an
        // upload against this mock would mint, so badges behave as on the real path.
        let scope = deviceInfo.libraryScope
        let base = Date()
        for (index, entry) in routes.enumerated() where !existing.contains(entry.summary.id) {
            var record = entry.record(addedAt: base.addingTimeInterval(-Double(index)), scope: scope)
            // A fixture the mock device holds boots up to date: the seeded fingerprint matches
            // what an upload of the record would send.
            if record.deviceLink != nil {
                record.uploadedCRC32 = RouteObjectCodec.payloadCRC(for: record)
            }
            store.savePlannedRoute(record)
        }
        // Trips group some of those routes. The routes are written above first, so no stage
        // dangles. Idempotent over trip ids, like the routes.
        let trips = lock.withLocked { _fixtures.trips }
        let existingTrips = Set(store.trips().map(\.id))
        for entry in trips where !existingTrips.contains(entry.id) {
            store.saveTrip(entry.record(base: base))
        }
    }

    // MARK: Transport-facing helpers

    /// Sleep the configured per-op latency.
    func delay() async {
        let duration = latency
        if duration > .zero { try? await Task.sleep(for: duration) }
    }

    /// The link is unusable only when down. `.outOfRange` still serves cached fixtures, because
    /// that state is content plus a banner, not an empty error screen.
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

    /// Radio and scan gate for the un-gated `discover()` phase. A `.timeout` pairing fault
    /// surfaces here, because the device never turns up in the scan window.
    func radioGate() throws {
        let (radio, pairing) = lock.withLocked { (_radio, _pairingFail) }
        switch radio {
        case .on: break
        case .off: throw DeviceError.bluetoothUnavailable(.poweredOff)
        case .unauthorized: throw DeviceError.bluetoothUnavailable(.unauthorized)
        }
        if pairing == .timeout { throw DeviceError.deviceNotFound }
    }

    /// Pairing gate for the gated `authenticate()` phase: a declined or wrong passkey, mapped
    /// onto `pairingFailed`. The UI keys the exact copy off `scenario`.
    func pairingGate() throws {
        if lock.withLocked({ _pairingFail }) == .rejected { throw DeviceError.pairingFailed }
    }

    /// Update the on-device config. A rename also updates the reported name.
    func setConfig(_ config: DeviceConfig) {
        lock.withLocked {
            _fixtures.config = config
            _fixtures.deviceInfo = _fixtures.deviceInfo.renamed(config.name)
        }
    }

    /// The device's route catalog: the fixture routes it holds a copy of, in the shape the
    /// protocol catalog produces. Reconcile input for the "on device" badge, never list rows.
    func deviceRoutes() -> [RouteCatalogEntry] {
        let routes = lock.withLocked { _fixtures.routes }
        let now = Date()
        var catalog = routes.compactMap { entry -> RouteCatalogEntry? in
            guard let objectID = entry.deviceObjectID else { return nil }
            // The catalog carries the whole-object CRC. A real upload pinned it; a seeded copy
            // derives it from the fixture geometry, so a device-held fixture boots up to date.
            let crc32 = entry.crc32 ?? RouteObjectCodec.payloadCRC(for: entry.record(addedAt: now))
            return RouteCatalogEntry(
                id: objectID, name: entry.summary.name,
                distanceMeters: entry.summary.distanceMeters,
                elevationGainMeters: entry.summary.elevationGainMeters,
                pointCount: entry.summary.pointCount,
                crc32: crc32
            )
        }
        // Resident-menu boundary hook: pad to one below the 64-route on-device snapshot. The
        // flat catalog still admits fresh objects, and the app must not conflate the two.
        if lock.withLocked({ _routesNearlyFull }) {
            let used = Set(catalog.map(\.id.raw))
            var filler = UInt64(50_000)
            while catalog.count < 63 {
                while used.contains(filler) { filler &+= 1 }
                catalog.append(RouteCatalogEntry(
                    id: DeviceObjectID(filler), name: "On-device route \(filler)",
                    distanceMeters: 0, elevationGainMeters: 0, pointCount: 0, crc32: 0))
                filler &+= 1
            }
        }
        return catalog
    }

    /// The stored copy behind a device object id.
    func deviceRouteEntry(_ id: DeviceObjectID) -> RouteEntry? {
        lock.withLocked { _fixtures.routes.first { $0.deviceObjectID == id } }
    }

    func recordSetClock(_ sample: WallClockSample) -> ClockSyncOutcome {
        lock.withLocked {
            // The command was sent whatever the answer, so record it first. An old-firmware
            // device answers `unknownCommand` and the app degrades gracefully.
            _setClockSamples.append(sample)
            return _supportsClockSync ? .stamped : .unsupported
        }
    }
    /// Record a `forgetBond` request. The mock models no device-side bond slot, so the count is
    /// the observable effect the forget tests assert.
    func recordForgetBond() {
        lock.withLocked { _forgetBondCount += 1 }
    }

    /// Non-zero means the connected forget reached the device before the local record was cleared.
    public var forgetBondCount: Int {
        lock.withLocked { _forgetBondCount }
    }

    public var cancelledRouteCatalogReadCount: Int {
        lock.withLocked { _cancelledRouteCatalogReadCount }
    }

    func recordCancelledRouteCatalogReadIfNeeded() {
        if Task.isCancelled { lock.withLocked { _cancelledRouteCatalogReadCount += 1 } }
    }

    /// Delete a stored route by its device object id: the device forgets its copy. The library
    /// record, and its list row, are the app's own business.
    func removeRoute(_ id: DeviceObjectID) {
        lock.withLocked {
            for index in _fixtures.routes.indices where _fixtures.routes[index].deviceObjectID == id {
                _fixtures.routes[index].deviceObjectID = nil
            }
        }
    }

    /// Simulate an on-device route delete: the device forgets its copy and notifies a catalog
    /// change, exactly the wire sequence the real firmware sends.
    public func deviceDeletesRoute(_ id: DeviceObjectID) {
        removeRoute(id)
        catalogChangedMulticast.send(CatalogChange(kind: .route))
    }

    // MARK: Trips (the device-side trip object store)

    /// Whether `deviceRoutes()` pads its catalog to the 64-route resident-menu boundary. The flat
    /// catalog stays writable: the menu snapshot is not physical storage.
    public var routesNearlyFull: Bool {
        get { lock.withLocked { _routesNearlyFull } }
        set { lock.withLocked { _routesNearlyFull = newValue } }
    }

    public var supportsClockSync: Bool {
        get { lock.withLocked { _supportsClockSync } }
        set { lock.withLocked { _supportsClockSync = newValue } }
    }

    /// Non-empty means the connect-time clock stamp reached the device.
    public var setClockSamples: [WallClockSample] {
        lock.withLocked { _setClockSamples }
    }
    /// One trip the mock device stores. `stageIDs` are device route object ids in ride order.
    struct DeviceTrip: Sendable {
        var id: DeviceObjectID
        var name: String
        var stageIDs: [DeviceObjectID]
        var payload: Data
        var crc32: UInt32
    }

    /// The device's trip catalog, one entry per stored trip, with stats summed over resolvable
    /// stages: a dangling stage counts in `stageCount` but not in the totals, as the firmware does.
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

    /// The stored trip object behind a device id: a byte-faithful decode of what an upload wrote.
    func deviceTripDecoded(_ id: DeviceObjectID) -> TripObjectCodec.Trip? {
        guard let trip = (lock.withLocked { _deviceTrips.first { $0.id == id } }) else { return nil }
        return try? TripObjectCodec.decode(trip.payload)
    }

    /// Begin a simulated trip upload. A new trip takes a fresh id from the trip counter. On commit
    /// the device records the copy, so a later `listTrips()` keeps the badge lit.
    func beginTripUpload(_ blob: TripBlob) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        if blob.payload.isEmpty { return .immediatelyFinished(.failed(.transferRejected)) }
        let isNew = blob.targetObjectID == nil
        // Fresh-upload dedup, mirroring the device's commit-time rule: identical content re-sent
        // as a new object, a retry after a lost commit ack, converges on the stored copy.
        let dedupID: DeviceObjectID? = !isNew ? nil : lock.withLocked {
            let crc = CRC32.checksum(blob.payload)
            return _deviceTrips.first { $0.crc32 == crc }?.id
        }
        let assignedID = AsyncPromise<DeviceObjectID?>()
        // A trip object is tiny, so pace off a small minimum and the progress bar stays visible.
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

    /// A committed trip upload landed: store or replace the copy under `objectID`, CRCing exactly
    /// the payload bytes received, so a re-list proves the badge against the committed fingerprint.
    private func recordDeviceTripCopy(of blob: TripBlob, objectID: DeviceObjectID) {
        let committedCRC = CRC32.checksum(blob.payload)
        let stored = DeviceTrip(
            id: objectID, name: blob.name, stageIDs: blob.deviceStageIDs,
            payload: blob.payload, crc32: committedCRC)
        lock.withLocked {
            if let index = _deviceTrips.firstIndex(where: { $0.id == objectID }) {
                _deviceTrips[index] = stored
            } else {
                _deviceTrips.append(stored)
            }
        }
    }

    /// Delete a stored trip by device id. Non-cascading: the trip metadata goes, member device
    /// routes stay. The id is recorded, so the cascade tests can assert the command landed.
    func removeTrip(_ id: DeviceObjectID) {
        lock.withLocked {
            _deviceTrips.removeAll { $0.id == id }
            _deletedTripObjectIDs.append(id)
        }
    }

    /// Record an app-issued route delete alongside the store mutation, so a test asserts both
    /// commands landed. `removeRoute` handles the store side.
    func recordRouteObjectDelete(_ id: DeviceObjectID) {
        lock.withLocked { _deletedRouteObjectIDs.append(id) }
    }

    /// The device object ids the app deleted this session.
    public var deletedRouteObjectIDs: [DeviceObjectID] { lock.withLocked { _deletedRouteObjectIDs } }
    public var deletedTripObjectIDs: [DeviceObjectID] { lock.withLocked { _deletedTripObjectIDs } }
    public var deviceTripCount: Int { lock.withLocked { _deviceTrips.count } }
    /// Lets a test drive a device-side trip delete against a real assigned id.
    public var deviceTripObjectIDs: [DeviceObjectID] { lock.withLocked { _deviceTrips.map(\.id) } }
    public func deviceTripStageIDs(_ id: DeviceObjectID) -> [DeviceObjectID] {
        lock.withLocked { _deviceTrips.first { $0.id == id }?.stageIDs ?? [] }
    }

    /// Simulate an on-device trip delete with cascade: the device forgets the trip and its member
    /// routes and notifies both stores, the wire sequence the real firmware sends.
    public func deviceDeletesTripCascade(_ id: DeviceObjectID) {
        let stageIDs: [DeviceObjectID] = lock.withLocked {
            let stages = _deviceTrips.first { $0.id == id }?.stageIDs ?? []
            _deviceTrips.removeAll { $0.id == id }
            return stages
        }
        for stageID in stageIDs { removeRoute(stageID) }
        catalogChangedMulticast.send(CatalogChange(kind: .route))
        catalogChangedMulticast.send(CatalogChange(kind: .trip))
    }

    /// Simulate a device-side trip-only delete: the app's trip link clears at reconcile while
    /// the member routes stay.
    public func deviceDeletesTrip(_ id: DeviceObjectID) {
        lock.withLocked { _deviceTrips.removeAll { $0.id == id } }
        catalogChangedMulticast.send(CatalogChange(kind: .trip))
    }

    /// Begin a simulated route upload. On commit it reports a device object id, fresh or the
    /// `targetObjectID` when replacing, and the fixture set records the copy so a later reconcile
    /// keeps the badge lit. Paced over a design-scale fiction of about 37 B/m, because a real
    /// route is only a few kB and its progress screen would flash by.
    func beginRouteUpload(_ blob: RouteBlob) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        if blob.payload.isEmpty { return .immediatelyFinished(.failed(.transferRejected)) }
        // Fresh-upload dedup, as in `beginTripUpload`: a re-sent new object whose payload CRC
        // matches an on-device copy answers with that copy's id instead of minting a twin. An
        // entry with no pinned CRC, a seeded fixture never uploaded, never matches.
        let dedupID: DeviceObjectID? = blob.targetObjectID != nil ? nil : lock.withLocked {
            let crc = CRC32.checksum(blob.payload)
            return _fixtures.routes.first { $0.deviceObjectID != nil && $0.crc32 == crc }?.deviceObjectID
        }
        let assignedID = AsyncPromise<DeviceObjectID?>()
        // Whichever is larger: an explicit test payload, or the design-scale minimum from the
        // route length, so a real few-kB route still paces long enough to see.
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

    // MARK: Firmware update

    /// What the next `installFw` request answers.
    public var firmwareInstallOutcome: FirmwareInstallResult {
        get { lock.withLocked { _firmwareInstallOutcome } }
        set { lock.withLocked { _firmwareInstallOutcome = newValue } }
    }

    /// Pace a firmware upload like a route push. On completion, remember the container's version
    /// so a modelled install reboot can reconnect the device onto it.
    func beginFirmwareUpload(_ container: Data) -> TransferHandle {
        if connection == .disconnected { return .immediatelyFinished(.failed(.notConnected)) }
        if container.isEmpty { return .immediatelyFinished(.failed(.transferRejected)) }
        let version = (try? StagedFirmware.validate(container))?.version
        // Pace off the container, but never so briefly that the progress bar cannot be seen.
        let pacingBytes = max(container.count, 850_000)
        let handle = startTransfer(total: pacingBytes, segments: [], rides: nil)
        Task { [weak self] in
            guard await handle.outcome == .completed else { return }
            self?.lock.withLocked { self?._firmwareStagedVersion = version }
        }
        return handle
    }

    /// Answer an `installFw` request with the configured outcome. On `accepted`, model the
    /// device's reboot: after a beat the link drops and comes back on the staged version.
    func installFirmware() -> FirmwareInstallResult {
        let outcome = lock.withLocked { _firmwareInstallOutcome }
        if outcome == .accepted { scheduleFirmwareReboot() }
        return outcome
    }

    /// Model the post-confirm reboot: drop the link, then reconnect reporting the staged firmware
    /// version. A no-op if nothing was staged this session.
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
                // Firmware replacement keeps the mounted store's identity.
                storeID: current.storeID,
                obcmVersion: current.obcmVersion
            )
            connection = .connecting
            try? await Task.sleep(for: .seconds(1))
            connection = .connected
        }
    }

    /// A committed upload landed: remember the copy in the fixture set, replacing the entry that
    /// already owns `objectID`, or the library twin of the same route.
    private func recordDeviceCopy(of blob: RouteBlob, objectID: DeviceObjectID) {
        // The device CRCs exactly the payload bytes it received, the same value the upload sheet
        // reports back to `markRouteUploaded`.
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

    /// Begin a simulated ride download: one paced batch whose fixture rides land as their bytes
    /// complete. The payload is the codec-encoded ride, so the consumer's decode is the real one.
    func beginRideDownload(_ ids: [RideID]) -> RideDownload {
        let wanted = Set(ids)
        let segments = lock.withLocked {
            _fixtures.rides.filter { wanted.contains($0.summary.id) }
        }.map {
            MockTransfer.Segment(id: $0.summary.id, byteCount: max(1, $0.downloadByteCount),
                                 payload: RideObjectCodec.encode($0.ride()))
        }
        let total = segments.reduce(0) { $0 + $1.byteCount }

        if connection == .disconnected { return .finished(.failed(.notConnected)) }
        if total == 0 { return .finished() }
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

// MARK: - NSLock convenience

extension NSLock {
    fileprivate func withLocked<T>(_ body: () -> T) -> T {
        lock(); defer { unlock() }; return body()
    }
}

extension DeviceInfo {
    fileprivate func renamed(_ name: String) -> DeviceInfo {
        DeviceInfo(name: name, firmwareVersion: firmwareVersion, hardwareVersion: hardwareVersion,
                   serial: serial, protocolVersion: protocolVersion, storeID: storeID,
                   obcmVersion: obcmVersion)
    }
}
#endif
