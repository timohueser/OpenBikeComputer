#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCDomain

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
/// Transfer discipline (spec §4.1/§4.2): **one transfer at a time** — every CoC
/// exchange (upload or download) holds the transport's transfer slot, writes its
/// descriptor only after the previous exchange's result landed, and treats the
/// device's closing `transferResult` as the *only* success signal. Interrupted
/// transfers **restart, not resume** (§1 principle 4).
///
/// All mutable state is confined to a single serial `queue` (the CoreBluetooth
/// callback queue); async methods hop onto it and register continuations that the
/// delegate callbacks resolve. That confinement is why this can be a plain
/// `@unchecked Sendable` class rather than fighting `Sendable` on CoreBluetooth's
/// (non-`Sendable`) object graph.
public final class BLETransport: NSObject, DeviceTransport, @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.openbikecomputer.ble")
    private lazy var central = CBCentralManager(delegate: self, queue: queue)

    private let stateMulticast = AsyncMulticast<ConnectionState>(.disconnected)
    /// `nil` until the first real BAS value — the seed must not replay as "0%".
    private let batteryMulticast = AsyncMulticast<Int?>(nil)
    /// `nil` seed = no replay: a `storeChanged` is an edge, not a state (the
    /// `storeChanges` doc on the protocol) — only live movements fan out.
    private let storeChangedMulticast = AsyncMulticast<StoreChanged?>(nil)

    private var peripheral: CBPeripheral?
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    /// The `(serial, epoch)` identity of the connected device, cached from the
    /// last successful `deviceInfo()` — what `listRides()` mints scoped
    /// `RideID`s with (#769). `nil` before the first identity read of a
    /// connection (and after a failed one): the catalog then mints legacy
    /// unscoped ids, which every scope-gated write ignores — and in practice
    /// the sync path can't reach `listRides()` in that state anyway, because
    /// the model's fail-closed gate opens only after an identity read that
    /// produced a scope. Cleared on disconnect: a reconnect may come back in a
    /// **new era** (chip erase, factory reset while away), and a stale scope
    /// must never key its catalog. Queue-confined like all mutable state.
    private var lastLibraryScope: LibraryScope?
    /// The live CoC byte pipe (`nil` until opened, or after a teardown). The
    /// `BLEChannel` wrapper is rebuilt around it on every (re)open.
    private var byteChannel: L2CAPByteChannel?
    private var bleChannel: BLEChannel?
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
    /// How long `forgetBond` waits for the device's `commandResult(ok)` before
    /// giving up (#756). Short by design — the device acks then immediately drops
    /// the link (which itself unblocks the ack waiter via the disconnect cleanup),
    /// and the forget is best-effort: a device that never answers must not stall
    /// the app's "Forget device" tap.
    private static let forgetBondAckTimeout: DispatchTimeInterval = .seconds(3)

    // Outstanding operations (all touched only on `queue`). Connecting is a
    // two-phase flow (#297): `discover()` (un-gated) then `authenticate()` (gated,
    // raises the passkey sheet) — each parks its own continuation.
    private var discoverContinuation: CheckedContinuation<Void, Error>?
    private var authenticateContinuation: CheckedContinuation<Void, Error>?
    private var wantsConnect = false
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

    // Device → app notifications, buffered so a waiter that registers just after a
    // notification arrives still sees it (no race with the write that provokes it) —
    // the same discipline the EchoHarness uses to drive this flow on glass. Only
    // solicited messages (transfer/command results) are buffered — unsolicited ones
    // (storeChanged) must not pile up for the session's life. Waiters fail and
    // buffers clear on disconnect.
    private var pendingStatuses: [StatusMessage] = []
    private var statusWaiters: [(pred: @Sendable (StatusMessage) -> Bool, cont: CheckedContinuation<StatusMessage, Error>)] = []
    private var pendingAnnounces: [TransferControl] = []
    private var announceWaiter: CheckedContinuation<TransferControl, Error>?

    // The one-transfer-at-a-time gate (spec §4.1) — reloads can't interleave a
    // list download with a running upload and cross their status notifications.
    private var transferBusy = false
    private var transferWaiters: [CheckedContinuation<Void, Never>] = []

    public override init() {
        super.init()
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
        // Same drop-the-seed pump as `battery`: subscribers see only movements
        // that happen after they subscribed, never the `nil` seed or a replay.
        let source = storeChangedMulticast.stream()
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
                wantsConnect = true
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
                wantsConnect = false
                if let peripheral { central.cancelPeripheralConnection(peripheral) }
                if central.isScanning { central.stopScan() }
                stateMulticast.send(.disconnected)
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
                wantsConnect = true
                startConnectIfReady()
                cont.resume()
            }
        }
    }

    // MARK: DeviceTransport — control plane

    public func deviceInfo() async throws -> DeviceInfo {
        async let fw = readString(GATT.firmwareRevision)
        async let hw = readString(GATT.hardwareRevision)
        async let serial = readString(GATT.serialNumber)
        // v2 read: `version u16 · store_epoch u32` LE (spec §1). The version field
        // keeps the **lenient prefix** decode (count >= 2 reads the first u16) —
        // that's the v1-peer compat path: a v1 device returns 2 bytes, reads as
        // `version = 1`, and takes the #303 mismatch banner. The epoch decode
        // instead requires the **full 6 bytes**; a short read leaves it `nil`
        // (unknown), never a fabricated `0` (`0` is a legal epoch). V5 (#769) gates
        // `ackRides`/reconcile on a present epoch — a `nil` here is that failed
        // identity read surfaced, not hidden behind a fake value.
        let versionData = try await read(GATT.protocolVersion)
        let b = versionData.startIndex
        let version = versionData.count >= 2 ? UInt16(versionData[b]) | (UInt16(versionData[b + 1]) << 8) : OBCProtocol.version
        let storeEpoch: UInt32? = versionData.count >= 6
            ? UInt32(versionData[b + 2]) | (UInt32(versionData[b + 3]) << 8)
                | (UInt32(versionData[b + 4]) << 16) | (UInt32(versionData[b + 5]) << 24)
            : nil
        let name = await currentPeripheralName() ?? "OBC"
        let info = DeviceInfo(
            name: name, firmwareVersion: try await fw, hardwareVersion: try await hw,
            serial: try await serial, protocolVersion: version, storeEpoch: storeEpoch
        )
        // Cache the scope for `listRides()` minting (#769). A read that carried
        // no epoch caches `nil` — deliberately: minting under a stale or absent
        // scope is the aliasing this whole mechanism exists to prevent.
        queue.async { [self] in lastLibraryScope = info.libraryScope }
        return info
    }

    public func readConfig() async throws -> DeviceConfig {
        try ConfigObjectCodec.decode(try await read(GATT.config))
    }

    public func writeConfig(_ config: DeviceConfig) async throws {
        try await write(ConfigObjectCodec.encode(config), to: GATT.config)
    }

    public func readDiagnostics() async throws -> Data {
        // Diagnostics are a CoC object (type 4, spec §7.5); the device serves it
        // from A7 — until then it answers a typed reject, which throws here.
        try await downloadObject(type: .diagnostics, objectID: 0)
    }

    public func deleteRoute(_ id: DeviceObjectID) async throws {
        // `deleteObject` (cmd 1): `cmd u8 · type u8 · object_id u16 LE` — spec §4.4.
        let payload = Data([1, ObjectType.route.rawValue, UInt8(id.raw & 0xFF), UInt8(id.raw >> 8)])
        // Hold the transfer slot (#302): `clearPendingStatuses` and `command` /
        // `transfer` results share one `pendingStatuses` buffer, so an ungated
        // delete could wipe a slot-holding transfer's buffered result out from
        // under it and hang that transfer forever. Serialize like the CoC exchanges.
        await acquireTransferSlot()
        defer { releaseTransferSlot() }
        clearPendingStatuses()
        try await write(payload, to: GATT.command)
        guard try await nextCommandResult().status == .ok else { throw DeviceError.writeFailed }
    }

    public func ackRides(_ ids: [RideID]) async throws {
        // `ackRides` (cmd 2): the possession list, chunked to the device's 64-byte
        // command value — spec §4.4. Only device-namespace ids can be acked (mock/
        // test ids that never came from a catalog have nothing to reconcile).
        let chunks = AckRidesCommand.chunks(ids.compactMap(\.deviceObjectID))
        guard !chunks.isEmpty else { return }
        // Hold the transfer slot across the whole batch, for `deleteRoute`'s
        // reason (#302): command results and transfer results share one
        // `pendingStatuses` buffer, so an ungated command write could wipe a
        // slot-holding transfer's buffered result out from under it.
        await acquireTransferSlot()
        defer { releaseTransferSlot() }
        clearPendingStatuses()
        for chunk in chunks {
            try await write(chunk, to: GATT.command)
            // Each chunk is answered on its own; the command is idempotent, so a
            // caller retrying after a mid-batch failure just re-sends everything.
            guard try await nextCommandResult().status == .ok else { throw DeviceError.writeFailed }
        }
    }

    public func listRoutes() async throws -> [RouteCatalogEntry] {
        // The `routeList` object (type 6, spec §7.4) over the CoC → the catalog.
        // Consumed for reconcile (the "on device" badge), never as list rows —
        // the Planned list is library-first (#289).
        let entries = try RouteList.decode(try await downloadObject(type: .routeList, objectID: 0))
        return entries.map { entry in
            RouteCatalogEntry(
                id: DeviceObjectID(entry.objectID),
                name: entry.name,
                distanceMeters: Double(entry.distanceMeters),
                elevationGainMeters: Double(entry.ascentMeters),
                pointCount: Int(entry.pointCount),
                crc32: entry.crc32
            )
        }
    }

    public func listRides() async throws -> RideCatalog {
        // The `rideList` object (type 7, spec §7.4) — the ride catalog (empty
        // until the firmware stores rides, A7). The v2 header's `total` surfaces
        // truncation past the device's `MAX_RIDES` cap (`hiddenRideCount`).
        //
        // Ids are minted **scoped** to the connected device's (serial, epoch)
        // identity (#769) — this is where the composite key enters the system;
        // everything downstream (coordinator, library, sets) stays id-string
        // driven. The scope comes from the identity read the connect flow runs
        // before any sync can start (the fail-closed gate).
        let scope = await withCheckedContinuation { (cont: CheckedContinuation<LibraryScope?, Never>) in
            queue.async { [self] in cont.resume(returning: lastLibraryScope) }
        }
        let decoded = try RideList.decode(try await downloadObject(type: .rideList, objectID: 0))
        let rides = decoded.entries.map { entry in
            RideSummary(
                id: scope.map { RideID(deviceObjectID: DeviceObjectID(entry.objectID), scope: $0) }
                    ?? RideID(deviceObjectID: DeviceObjectID(entry.objectID)),
                name: entry.name,
                date: Date(timeIntervalSince1970: TimeInterval(entry.startTime)),
                distanceMeters: Double(entry.distanceMeters),
                movingTime: TimeInterval(entry.movingTimeSeconds),
                averageSpeedMps: Double(entry.averageSpeedCms) / 100,
                climbMeters: Double(entry.climbMeters)
            )
        }
        return RideCatalog(rides: rides, hiddenRideCount: decoded.hiddenCount)
    }

    public func routeDetail(_ id: DeviceObjectID) async throws -> RouteDetail {
        // Pinned by S0 as "download the route object" (spec §7.1): the stored OBCR
        // v2 blob, decoded app-side for the waypoints + elevation profile — one
        // layout, one truth.
        let decoded = try RouteObjectCodec.decode(try await downloadObject(type: .route, objectID: id.raw))
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
        // The `tripList` object (type 10, spec §7.4) over the CoC → the trip
        // catalog. Reconcile-only (the trip card's "on device" badge, TR8), never
        // list rows — trips are library-first like routes.
        try TripList.catalog(try await downloadObject(type: .tripList, objectID: 0))
    }

    public func downloadTrip(_ id: DeviceObjectID) async throws -> TripObjectCodec.Decoded {
        // "Download the trip object" (spec §7.7) — the stored trip blob, decoded
        // app-side for its name + stage ids. Reconcile falls back to it only when
        // the `tripList` `crc32` can't confirm the fingerprint.
        try TripObjectCodec.decode(try await downloadObject(type: .trip, objectID: id.raw))
    }

    public func deleteTrip(_ id: DeviceObjectID) async throws {
        // `deleteObject` for a trip (cmd 1, type 9) — **non-cascading** (spec §4.4:
        // the trip metadata goes, member routes stay); the "Delete trip & routes"
        // cascade is composed by the caller. Same transfer-slot serialization as
        // `deleteRoute` (#302 — command and transfer results share one buffer).
        let payload = Data([1, ObjectType.trip.rawValue, UInt8(id.raw & 0xFF), UInt8(id.raw >> 8)])
        await acquireTransferSlot()
        defer { releaseTransferSlot() }
        clearPendingStatuses()
        try await write(payload, to: GATT.command)
        guard try await nextCommandResult().status == .ok else { throw DeviceError.writeFailed }
    }

    // MARK: DeviceTransport — data plane

    public func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        // A fresh upload sends objectID 0xFFFF = "new" (the device assigns one);
        // re-uploading an edited route sends its stored id, which replaces that
        // object in place (spec §4.1/§4.2). Success is the device's closing
        // `transferResult` — never the local byte flush.
        guard !route.payload.isEmpty else {
            // Nothing to send is a caller bug (a route without geometry must not
            // offer Upload) — fail loudly instead of "committing" nothing.
            return .immediatelyFinished(.failed(.transferRejected))
        }
        let descriptor = TransferControl(
            op: .upload, type: .route, objectID: route.targetObjectID?.raw ?? TransferControl.newObjectID,
            totalLen: UInt32(route.payload.count), crc32: CRC32.checksum(route.payload)
        )
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let assignedID = AsyncPromise<DeviceObjectID?>()
        let runner = UploadRunner(
            transport: self, payload: route.payload, descriptor: descriptor,
            progress: continuation, outcome: outcome, assignedID: assignedID
        )
        Task { await runner.start() }
        return TransferHandle(
            progress: stream,
            outcome: outcome,
            assignedObjectID: assignedID,
            onCancel: { Task { await runner.cancel() } },
            onResume: { Task { await runner.start() } }
        )
    }

    public func uploadTrip(_ trip: TripBlob) -> TransferHandle {
        // The trip object (type 9) — the trip sibling of `uploadRoute`, reusing
        // the same runner; only the descriptor's type differs. Fresh sends 0xFFFF
        // (the device mints a trip id); a re-push / adoption sends the stored id
        // to replace in place (spec §4.1/§4.2). Uploaded last in a whole-trip push.
        guard !trip.payload.isEmpty else {
            return .immediatelyFinished(.failed(.transferRejected))
        }
        let descriptor = TransferControl(
            op: .upload, type: .trip, objectID: trip.targetObjectID?.raw ?? TransferControl.newObjectID,
            totalLen: UInt32(trip.payload.count), crc32: CRC32.checksum(trip.payload)
        )
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let assignedID = AsyncPromise<DeviceObjectID?>()
        let runner = UploadRunner(
            transport: self, payload: trip.payload, descriptor: descriptor,
            progress: continuation, outcome: outcome, assignedID: assignedID
        )
        Task { await runner.start() }
        return TransferHandle(
            progress: stream,
            outcome: outcome,
            assignedObjectID: assignedID,
            onCancel: { Task { await runner.cancel() } },
            onResume: { Task { await runner.start() } }
        )
    }

    public func uploadFirmware(_ container: Data) -> TransferHandle {
        // A `fwImage` upload (spec §7.6): the whole OBCU container, the singleton
        // object id `0` (the device assigns none and echoes `0`). Reuses the route
        // upload's runner — the descriptor is the only difference. Success is the
        // device's committed `transferResult`, which promotes the bytes to
        // `/UPDATE.BIN`; staging never installs (that's `installFirmware`).
        guard !container.isEmpty else {
            return .immediatelyFinished(.failed(.transferRejected))
        }
        let descriptor = TransferControl(
            op: .upload, type: .firmware, objectID: 0,
            totalLen: UInt32(container.count), crc32: CRC32.checksum(container)
        )
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        // The singleton stage assigns no id; the runner still takes a promise (it
        // fulfills it with the echoed `0`), which no firmware caller reads.
        let assignedID = AsyncPromise<DeviceObjectID?>()
        let runner = UploadRunner(
            transport: self, payload: container, descriptor: descriptor,
            progress: continuation, outcome: outcome, assignedID: assignedID
        )
        Task { await runner.start() }
        return TransferHandle(
            progress: stream,
            outcome: outcome,
            onCancel: { Task { await runner.cancel() } },
            onResume: { Task { await runner.start() } }
        )
    }

    public func installFirmware() async throws -> FirmwareInstallResult {
        // `installFw` (cmd 3): the `cmd` byte only — spec §4.4. Holds the transfer
        // slot for `deleteRoute`'s reason (#302): command and transfer results
        // share one `pendingStatuses` buffer, so an ungated command write could
        // wipe a slot-holding transfer's buffered result. The command returns as
        // soon as the request is accepted; it never waits for the on-glass confirm.
        await acquireTransferSlot()
        defer { releaseTransferSlot() }
        clearPendingStatuses()
        try await write(Data([3]), to: GATT.command)
        return FirmwareInstallResult(commandStatus: try await nextCommandResult().status)
    }

    public func forgetBond() async throws {
        // `forgetBond` (cmd 4): the `cmd` byte only — spec §4.4. The app's "Forget
        // device" asks the device to dissolve ITS side of the bond too, so a one-
        // sided app forget doesn't leave the pair wedged: the device's reject-when-
        // bonded posture (spec §8) would otherwise refuse every new pairing until
        // the rider ran Forget phone on the device. Reachable only over the bonded,
        // encrypted link (the gated `command` characteristic requires it), so a
        // stranger can never issue it. Holds the transfer slot for `deleteRoute`'s
        // reason (#302): command and transfer results share one `pendingStatuses`
        // buffer. The device answers `commandResult(ok)` first, then drops the link
        // — so we wait only briefly for the ack; a timeout or write failure throws,
        // and the caller (Settings forget) treats this as best-effort and clears
        // its local record regardless.
        await acquireTransferSlot()
        defer { releaseTransferSlot() }
        clearPendingStatuses()
        try await write(Data([4]), to: GATT.command)
        // The ack timeout is a queue-confined one-shot (the watchdog idiom above),
        // deliberately NOT a task-group race: `nextCommandResult()`'s parked
        // continuation is not cancellation-responsive — it resolves only on a
        // matching status or the disconnect cleanup — so a throwing group would
        // sit un-drainable on the losing child exactly when the device never
        // answers (the same wedge `LaunchFlowModel` documents for racing
        // `connect()`). Instead the one-shot unwinds the waiter through the same
        // resume-throwing path the disconnect cleanup uses. Failing ALL parked
        // status waiters is safe here: every status-waiting op parks only while
        // holding the transfer slot, and we hold it — the only parked waiter is
        // ours. The defer cancels the one-shot before the slot is released (LIFO),
        // and a lost race is harmless either way: on the serial `queue`, a fire
        // after our waiter resolved finds `statusWaiters` empty and no-ops.
        let ackTimeout = DispatchWorkItem { [weak self] in
            self?.failStatusWaiters(DeviceError.writeFailed)
        }
        queue.asyncAfter(deadline: .now() + Self.forgetBondAckTimeout, execute: ackTimeout)
        defer { ackTimeout.cancel() }
        _ = try await nextCommandResult()
    }

    /// Unwind every parked `status` waiter with `error` — `forgetBond`'s ack
    /// timeout. Queue-confined: called only from a work item already running on
    /// `queue` (`asyncAfter` executes on its target).
    private func failStatusWaiters(_ error: DeviceError) {
        dispatchPrecondition(condition: .onQueue(queue))
        let waiters = statusWaiters
        statusWaiters.removeAll()
        for waiter in waiters { waiter.cont.resume(throwing: error) }
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
            onResume: { Task { await runner.start() } }
        )
        return RideDownload(handle: handle, rides: rideStream)
    }

    // MARK: One object over the CoC (queue-confined helpers around it)

    /// Download one object (spec §4.2 op 2): take the transfer slot, write the
    /// request, await the device's announce descriptor (`total_len` + `crc32`) —
    /// a typed reject resolves it as a throw — stream the payload off the CoC
    /// verifying the whole-object CRC, then require the committed close.
    fileprivate func downloadObject(type: ObjectType, objectID: UInt16) async throws -> Data {
        await acquireTransferSlot()
        defer { releaseTransferSlot() }
        let channel = try await readyChannel()
        clearPendingStatuses()
        try await write(TransferControl(op: .download, type: type, objectID: objectID).encode(), to: GATT.transferControl)
        let announce = try await nextAnnounce()
        let bytes = try await channel.receive(length: Int(announce.totalLen), expectedCRC: announce.crc32)
        guard try await nextTransferResult().status == .committed else { throw DeviceError.readFailed }
        return bytes
    }

    /// The transfer slot (spec §4.1: one transfer in flight). Holders release at
    /// the end of each attempt, so a stalled (drop-waiting) upload doesn't starve
    /// reconnect-time list reads.
    fileprivate func acquireTransferSlot() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                if transferBusy { transferWaiters.append(cont) } else {
                    transferBusy = true
                    cont.resume()
                }
            }
        }
    }

    fileprivate func releaseTransferSlot() {
        queue.async { [self] in
            if transferWaiters.isEmpty {
                transferBusy = false
            } else {
                transferWaiters.removeFirst().resume()  // hand the slot over
            }
        }
    }

    /// Drop buffered results/announces from a previous exchange before writing a
    /// fresh descriptor — a stale `aborted` (e.g. from a canceled upload's channel
    /// teardown) must never be read as this exchange's answer. Safe because the
    /// device can't answer a descriptor before it is written.
    fileprivate func clearPendingStatuses() {
        queue.async { [self] in
            pendingStatuses.removeAll()
            pendingAnnounces.removeAll()
        }
    }

    /// The live CoC channel, (re)opening it if the previous one was torn down
    /// (a canceled transfer closes the channel so the device discards its partial).
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

    /// Tear the CoC down (a canceled transfer): the device sees the drop, discards
    /// its partial, and re-listens; the next transfer re-opens via `readyChannel`.
    fileprivate func teardownChannel() async {
        let channel: L2CAPByteChannel? = queue.sync {
            let current = byteChannel
            byteChannel = nil
            bleChannel = nil
            return current
        }
        await channel?.close()
    }

    // MARK: Status / announce notifications (queue-confined)

    private func deliverStatus(_ message: StatusMessage) {
        // A transferResult while a download announce is awaited IS the answer to
        // that request — a typed reject (notFound / busy / error), spec §4.2.
        if case .transferResult(let result) = message, let waiter = announceWaiter {
            announceWaiter = nil
            waiter.resume(throwing: rejectError(result.status))
            return
        }
        // v2: the download announce rides `status` (`msg = 4`) — route it to the
        // announce waiter/buffer, the single ordering domain folding it into
        // `status` buys us (the split-CCCD failure mode is gone with the
        // `transferControl` CCCD).
        if case .downloadAnnounce(let descriptor) = message {
            deliverAnnounce(descriptor)
            return
        }
        if let index = statusWaiters.firstIndex(where: { $0.pred(message) }) {
            statusWaiters.remove(at: index).cont.resume(returning: message)
            return
        }
        switch message {
        case .transferResult, .commandResult:
            pendingStatuses.append(message)
        case .storeChanged(let change):
            // Unsolicited → never buffered (it must not pile up for the
            // session's life), but fanned out live so the main screen can
            // re-reconcile its "on device" badges when the device's store
            // moves under an open app (on-device delete, epic #447 P6).
            storeChangedMulticast.send(change)
        case .downloadAnnounce, .unknown:
            break  // announce is handled above (unreachable here); unknowns aren't buffered
        }
    }

    private func rejectError(_ status: TransferResult.Status) -> DeviceError {
        switch status {
        case .crcMismatch: .crcMismatch
        case .storageFull: .storageFull
        // committed/aborted aren't rejects; if one ever reaches here it's a
        // generic device-side failure like the rest.
        case .committed, .aborted, .error, .notFound, .busy: .transferRejected
        }
    }

    private func deliverAnnounce(_ descriptor: TransferControl) {
        if let waiter = announceWaiter {
            announceWaiter = nil
            waiter.resume(returning: descriptor)
        } else {
            pendingAnnounces.append(descriptor)
        }
    }

    private func nextStatus(where predicate: @escaping @Sendable (StatusMessage) -> Bool) async throws -> StatusMessage {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<StatusMessage, Error>) in
            queue.async { [self] in
                if let index = pendingStatuses.firstIndex(where: predicate) {
                    cont.resume(returning: pendingStatuses.remove(at: index))
                } else {
                    statusWaiters.append((predicate, cont))
                }
            }
        }
    }

    fileprivate func nextTransferResult() async throws -> TransferResult {
        guard case .transferResult(let result) = try await nextStatus(where: {
            if case .transferResult = $0 { true } else { false }
        }) else { fatalError("predicate guarantees a transferResult") }
        return result
    }

    private func nextCommandResult() async throws -> CommandResult {
        guard case .commandResult(let result) = try await nextStatus(where: {
            if case .commandResult = $0 { true } else { false }
        }) else { fatalError("predicate guarantees a commandResult") }
        return result
    }

    private func nextAnnounce() async throws -> TransferControl {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<TransferControl, Error>) in
            queue.async { [self] in
                if !pendingAnnounces.isEmpty {
                    cont.resume(returning: pendingAnnounces.removeFirst())
                } else if let index = pendingStatuses.firstIndex(where: {
                    if case .transferResult = $0 { true } else { false }
                }) {
                    // A buffered transferResult with no announce ahead of it is a
                    // typed reject answering *this* download — a successful download
                    // always notifies the announce first (spec §4.2). It lands in
                    // `pendingStatuses` when the reject beats this waiter's
                    // registration (the write-ack → nextAnnounce hop); `deliverStatus`
                    // only resolves the announce-as-reject case when a waiter is
                    // already parked, so without draining it here the reject sits
                    // buffered forever and hangs `downloadObject`.
                    guard case .transferResult(let result) = pendingStatuses.remove(at: index) else {
                        fatalError("predicate guarantees a transferResult")
                    }
                    cont.resume(throwing: rejectError(result.status))
                } else {
                    announceWaiter = cont
                }
            }
        }
    }

    // MARK: Connect flow (queue-confined)

    private func startConnectIfReady() {
        guard wantsConnect else { return }
        switch central.state {
        case .poweredOn:
            stateMulticast.send(.connecting)
            central.scanForPeripherals(withServices: [GATT.obcControlService])
        case .poweredOff:
            failDiscover(.bluetoothUnavailable(.poweredOff))
        case .unauthorized:
            failDiscover(.bluetoothUnavailable(.unauthorized))
        case .unsupported:
            failDiscover(.bluetoothUnavailable(.unsupported))
        default:
            break  // .resetting / .unknown → wait for the next state update
        }
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
        // v2: one notify surface — `status` alone. `transferControl` is write-only
        // (the download announce it once notified now rides `status` as `msg = 4`),
        // so its CCCD subscribe path — and the split-CCCD ordering it created — is gone.
        if let statusCharacteristic = characteristics[GATT.status] {
            peripheral.setNotifyValue(true, for: statusCharacteristic)
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
            if self.discoverContinuation != nil { self.failDiscover(.deviceNotFound) }
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
        wantsConnect = false
        stateMulticast.send(.disconnected)
        discoverContinuation?.resume(throwing: error)
        discoverContinuation = nil
    }

    /// Phase 2 failed (declined passkey / refused encryption / CoC open) — tear the
    /// intent down so a background reconnect doesn't spin on a bond that won't take.
    private func failAuthenticate(_ error: DeviceError) {
        disarmChannelWatchdog()
        awaitingGatedRetry = false
        wantsConnect = false
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
                wantsConnect = false
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

    /// #753 — the one shared retry proxy for a failed *gated op* (a `status` /
    /// `transferControl` CCCD write or the PSM read), used by both delegate
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
        // Only announce an actual transition (#302): a mid-session CoC reopen
        // (after a canceled-transfer `teardownChannel`) re-enters here, but the
        // link never left `.connected` — re-sending would re-fire edge-triggered
        // observers. The authenticate continuation still resolves unconditionally
        // (a fresh `authenticate()` completes here regardless of the state edge).
        if stateMulticast.value != .connected { stateMulticast.send(.connected) }
        awaitingGatedRetry = false
        authenticateContinuation?.resume()
        authenticateContinuation = nil
    }

    /// The link is gone: every parked continuation must resolve (a leaked
    /// `CheckedContinuation` hangs its caller forever), and buffered notifications
    /// from the dead link are dropped (a new connection re-announces).
    private func failAllPending() {
        let reads = pendingReads.values.flatMap { $0 }
        pendingReads.removeAll()
        let writes = pendingWrites.values.flatMap { $0 }
        pendingWrites.removeAll()
        let statuses = statusWaiters
        statusWaiters.removeAll()
        let announce = announceWaiter
        announceWaiter = nil
        let channels = channelWaiters
        channelWaiters.removeAll()
        pendingStatuses.removeAll()
        pendingAnnounces.removeAll()
        openingChannel = false
        disarmChannelWatchdog()
        for cont in reads { cont.resume(throwing: DeviceError.notConnected) }
        for cont in writes { cont.resume(throwing: DeviceError.notConnected) }
        for waiter in statuses { waiter.cont.resume(throwing: DeviceError.notConnected) }
        announce?.resume(throwing: DeviceError.notConnected)
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

/// One route upload from `uploadRoute` to its terminal outcome. An interrupted
/// attempt (link/CoC drop) leaves the outcome unresolved — the F sheet shows
/// "interrupted" via `DeviceTransport.state` — and `start()` (the handle's
/// `resume()`) runs a **whole fresh attempt**: descriptor + all bytes from 0
/// (uploads restart, not resume — spec §1 principle 4).
private actor UploadRunner {
    private let transport: BLETransport
    private let payload: Data
    private let descriptor: TransferControl
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>
    private let assignedID: AsyncPromise<DeviceObjectID?>
    private var attempt: Task<Void, Never>?

    init(
        transport: BLETransport, payload: Data, descriptor: TransferControl,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>, assignedID: AsyncPromise<DeviceObjectID?>
    ) {
        self.transport = transport
        self.payload = payload
        self.descriptor = descriptor
        self.progress = progress
        self.outcome = outcome
        self.assignedID = assignedID
    }

    /// Start (or restart after a drop). No-op while an attempt runs or after a
    /// terminal outcome.
    func start() {
        guard attempt == nil, outcome.current == nil else { return }
        attempt = Task {
            await runAttempt()
            attempt = nil
        }
    }

    /// Abort: stop the pump and tear the CoC down — the device sees the drop and
    /// discards its partial (its `aborted` result is cleared before the next
    /// exchange's descriptor). Terminal.
    func cancel() async {
        attempt?.cancel()
        await transport.teardownChannel()
        finish(.canceled)
    }

    private func runAttempt() async {
        await transport.acquireTransferSlot()
        defer { transport.releaseTransferSlot() }
        guard outcome.current == nil else { return }

        // No channel and no way to open one = no link at all (H4): terminal.
        let channel: BLEChannel
        do {
            channel = try await transport.readyChannel()
        } catch {
            finish(.failed((error as? DeviceError) ?? .notConnected))
            return
        }

        do {
            transport.clearPendingStatuses()
            try await transport.write(descriptor.encode(), to: GATT.transferControl)
            let ticks = progress
            try await channel.send(payload) { ticks.yield($0) }
            // Bytes flushed — now the only signal that counts: the device's verdict.
            let result = try await transport.nextTransferResult()
            switch result.status {
            case .committed:
                assignedID.fulfill(result.objectID)
                finish(.completed)
            case .crcMismatch:
                finish(.failed(.crcMismatch))
            case .aborted:
                finish(.canceled)  // our cancel raced the completion
            case .storageFull:
                finish(.failed(.storageFull))  // catalog full — a new-route reject
            case .error, .notFound, .busy:
                finish(.failed(.transferRejected))
            }
        } catch is CancellationError {
            // cancel() resolves the outcome and tears the channel down.
        } catch DeviceError.writeFailed {
            finish(.failed(.writeFailed))  // GATT rejected the descriptor, link up
        } catch {
            // The link/CoC dropped mid-attempt: stay unresolved — resumable. The
            // device discards its partial; the next start() re-sends everything.
        }
    }

    private func finish(_ terminal: TransferOutcome) {
        progress.finish()
        outcome.fulfill(terminal)
        if terminal != .completed { assignedID.fulfill(nil) }
    }
}

/// Drives a ride-sync batch (A7): downloads each requested ride object over the
/// CoC and yields it the instant its bytes are complete + CRC-verified, so a drop
/// keeps every ride already yielded and `resume()` re-requests only the rest —
/// whole rides are the batch's elementary unit (spec §1 principle 4, "multi-object
/// flows resume at whole-object granularity"). The download-object plumbing
/// (`downloadObject`) owns the per-ride transfer slot, so a stalled batch never
/// starves reconnect-time list reads. Mirrors `UploadRunner`'s lifecycle.
private actor RideDownloadRunner {
    private let transport: BLETransport
    /// Each requested ride with its device object id, resolved (and validated)
    /// by `downloadRides` before the runner exists.
    private let requests: [(id: RideID, objectID: DeviceObjectID)]
    private let rides: AsyncThrowingStream<DownloadedRide, Error>.Continuation
    private let progress: AsyncStream<TransferProgress>.Continuation
    private let outcome: AsyncPromise<TransferOutcome>
    private var landed: Set<RideID> = []
    private var attempt: Task<Void, Never>?
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

    /// Start (or restart after a drop). No-op while an attempt runs or after a
    /// terminal outcome; a restart continues at the first not-yet-landed ride.
    func start() {
        guard attempt == nil, !finished else { return }
        attempt = Task {
            await runAttempt()
            attempt = nil
        }
    }

    /// Abort the batch: stop the pump and tear the CoC down so the device discards
    /// any in-flight partial. Terminal — rides already yielded stay landed.
    func cancel() async {
        attempt?.cancel()
        await transport.teardownChannel()
        finish(.canceled)
    }

    private func runAttempt() async {
        guard !finished else { return }
        do {
            for (id, objectID) in requests where !landed.contains(id) {
                try Task.checkCancellation()
                let payload = try await transport.downloadObject(type: .ride, objectID: objectID.raw)
                landed.insert(id)
                rides.yield(DownloadedRide(id: id, payload: payload))
                progress.yield(TransferProgress(bytesDone: landed.count, total: requests.count))
            }
            finish(.completed)
        } catch is CancellationError {
            // cancel() resolves the outcome and tears the channel down.
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
            // A link/CoC drop mid-batch (or a reconnect-needed): stay unresolved —
            // resumable. What landed is kept (already yielded + persisted); the
            // sync's drop watch raises H10 and `resume()` re-requests the rest.
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

// MARK: - CBCentralManagerDelegate

extension BLETransport: CBCentralManagerDelegate {
    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        startConnectIfReady()
    }

    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
                               advertisementData: [String: Any], rssi RSSI: NSNumber) {
        central.stopScan()
        self.peripheral = peripheral
        peripheral.delegate = self
        central.connect(peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        armDiscoveryWatchdog()  // bounds GATT discovery, not the scan/reconnect wait (#302)
        peripheral.discoverServices([GATT.deviceInformation, GATT.battery, GATT.obcControlService])
    }

    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        if discoverContinuation != nil {
            failDiscover(.notConnected)
        } else if wantsConnect {
            // A background reconnect attempt failed — keep trying; the request
            // sits pending in the controller until the device reappears.
            central.connect(peripheral)
        }
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        characteristics.removeAll()
        // The scope dies with the link: the device may come back in a new id
        // era (#769) — the next connection's identity read re-establishes it.
        lastLibraryScope = nil
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
            wantsConnect = false
            stateMulticast.send(.disconnected)
            return
        }
        stateMulticast.send(wantsConnect ? .outOfRange : .disconnected)
        // Reconnect (S4: the banner degrades, the link keeps trying): a connect
        // issued now has no timeout — iOS holds it pending until the peripheral
        // advertises again, then the normal didConnect → discovery → CoC flow
        // publishes .connected. `disconnect()` cancels it via
        // cancelPeripheralConnection.
        if wantsConnect { central.connect(peripheral) }
    }
}

// MARK: - CBPeripheralDelegate

extension BLETransport: CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil else {
            failDiscover(.notConnected)
            return
        }
        let services = peripheral.services ?? []
        pendingServiceDiscovery = services.count
        for service in services {
            peripheral.discoverCharacteristics(nil, for: service)
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard error == nil else {
            failDiscover(.notConnected)
            return
        }
        for characteristic in service.characteristics ?? [] {
            characteristics[characteristic.uuid] = characteristic
            // Only the **un-gated** BAS notify is armed here (#297). The gated
            // `status` / `transferControl` notifies and the PSM read wait for
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
        // Every service's characteristics are in hand — the un-gated surface is
        // ready. A pending `discover()` resolves here (its caller runs
        // `authenticate()` next, on the D2 row tap); an unsolicited background
        // reconnect (bonded, no waiter) proceeds straight to the gated phase to
        // restore the full link.
        if let cont = discoverContinuation {
            discoverContinuation = nil
            cont.resume()
        } else {
            beginAuthenticate()
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
        // Typed device → app `status` messages — the sole device → app channel in
        // v2 (transferResult / storeChanged / commandResult / **downloadAnnounce**).
        // The download announce arrives here as `msg = 4`; `deliverStatus` routes
        // it to the announce waiter (`transferControl` is write-only now).
        if uuid == GATT.status {
            if let data = characteristic.value, let message = try? StatusMessage(decoding: data) { deliverStatus(message) }
            return
        }

        // Resolve a pending read.
        resumeReads(uuid, error == nil ? .success(characteristic.value ?? Data()) : .failure(DeviceError.readFailed))
    }

    public func peripheral(
        _ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?
    ) {
        // #753: on a fresh pair the FIRST gated op is a CCCD write, not the PSM
        // read — `beginAuthenticate` arms the `status` notify before reading the
        // PSM — so it's the CCCD write that raises the passkey sheet, and iOS's
        // post-passkey replay of the gated ops hits the CCCD write *first*. The
        // firmware's post-PairingComplete refusal window therefore most likely
        // clips the CCCD write; without this handler that failure was silently
        // swallowed — and if the window then drained before the PSM read,
        // `authenticate()` resolved with a DEAD `status` notify (no transferResult
        // or announce would ever arrive). Map it exactly like the PSM branch: the
        // shared retryable proxy → resolve the parked authenticate as retryable,
        // and the retry's `beginAuthenticate` re-arms the gated ops. Everything
        // else keeps the pre-existing ignore: a background re-arm has no
        // authenticate pending, and a real decline tears the link down
        // (`didDisconnectPeripheral` owns that path). No notify-state bookkeeping
        // beyond this. (v2: `transferControl` is write-only — only `status` is
        // notified now, so it's the sole CCCD this window can clip.)
        guard characteristic.uuid == GATT.status else { return }
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
