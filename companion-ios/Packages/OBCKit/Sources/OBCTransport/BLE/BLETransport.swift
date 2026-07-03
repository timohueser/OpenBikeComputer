#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCDomain

/// The **real** `DeviceTransport` (Tier 1 → CoreBluetooth). Scans for the OBC
/// service, connects, discovers DIS/BAS/OBC Control, reads the PSM and opens the
/// L2CAP CoC, and maps the semantic protocol onto GATT reads/writes/notifies +
/// the `BLEChannel` byte layer.
///
/// Route/ride ids on this transport are **device-namespace**: a `RouteID`/`RideID`
/// whose `rawValue` is the decimal object id from the device's list objects
/// (spec §4.1). The app's library ids never cross this boundary — the link between
/// a library route and its device copy is the persisted `deviceObjectID`.
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
    private let batteryMulticast = AsyncMulticast<Int>(0)

    private var peripheral: CBPeripheral?
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    /// The live CoC byte pipe (`nil` until opened, or after a teardown). The
    /// `BLEChannel` wrapper is rebuilt around it on every (re)open.
    private var byteChannel: L2CAPByteChannel?
    private var bleChannel: BLEChannel?
    private var openingChannel = false
    private var channelWaiters: [CheckedContinuation<BLEChannel, Error>] = []

    // Outstanding operations (all touched only on `queue`).
    private var connectContinuation: CheckedContinuation<Void, Error>?
    private var wantsConnect = false
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
    public var battery: AsyncStream<Int> { batteryMulticast.stream() }

    public func connect() async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                connectContinuation = cont
                wantsConnect = true
                startConnectIfReady()
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

    // MARK: DeviceTransport — control plane

    public func deviceInfo() async throws -> DeviceInfo {
        async let fw = readString(GATT.firmwareRevision)
        async let hw = readString(GATT.hardwareRevision)
        async let serial = readString(GATT.serialNumber)
        let versionData = try await read(GATT.protocolVersion)
        let version = versionData.count >= 2 ? UInt16(versionData[0]) | (UInt16(versionData[1]) << 8) : OBCProtocol.version
        let name = await currentPeripheralName() ?? "OBC"
        return DeviceInfo(
            name: name, firmwareVersion: try await fw, hardwareVersion: try await hw,
            serial: try await serial, protocolVersion: version
        )
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

    public func deleteRoute(_ id: RouteID) async throws {
        // `deleteObject` (cmd 1): `cmd u8 · type u8 · object_id u16 LE` — spec §4.4.
        // `id` is device-namespace (a decimal object id from `listRoutes`).
        guard let objectID = UInt16(id.rawValue) else { throw DeviceError.writeFailed }
        let payload = Data([1, ObjectType.route.rawValue, UInt8(objectID & 0xFF), UInt8(objectID >> 8)])
        clearPendingStatuses()
        try await write(payload, to: GATT.command)
        guard try await nextCommandResult().status == .ok else { throw DeviceError.writeFailed }
    }

    public func listRoutes() async throws -> [RouteSummary] {
        // The `routeList` object (type 6, spec §7.4) over the CoC → the catalog.
        // Consumed for reconcile (the "on device" badge), never as list rows —
        // the Planned list is library-first (#289).
        let entries = try RouteList.decode(try await downloadObject(type: .routeList, objectID: 0))
        return entries.map { entry in
            RouteSummary(
                id: RouteID(String(entry.objectID)),
                name: entry.name,
                distanceMeters: Double(entry.distanceMeters),
                elevationGainMeters: Double(entry.ascentMeters),
                pointCount: Int(entry.pointCount)
            )
        }
    }

    public func listRides() async throws -> [RideSummary] {
        // The `rideList` object (type 7, spec §7.4) — the ride catalog (empty
        // until the firmware stores rides, A7).
        let entries = try RideList.decode(try await downloadObject(type: .rideList, objectID: 0))
        return entries.map { entry in
            RideSummary(
                id: RideID(String(entry.objectID)),
                name: entry.name,
                date: Date(timeIntervalSince1970: TimeInterval(entry.startTime)),
                distanceMeters: Double(entry.distanceMeters),
                movingTime: TimeInterval(entry.movingTimeSeconds),
                averageSpeedMps: Double(entry.averageSpeedCms) / 100,
                climbMeters: Double(entry.climbMeters)
            )
        }
    }

    public func routeDetail(_ id: RouteID) async throws -> RouteDetail {
        // Pinned by S0 as "download the route object" (spec §7.1): the stored OBCR
        // v2 blob, decoded app-side for the waypoints + elevation profile — one
        // layout, one truth. `id` is device-namespace.
        guard let objectID = UInt16(id.rawValue) else { throw DeviceError.readFailed }
        let decoded = try RouteObjectCodec.decode(try await downloadObject(type: .route, objectID: objectID))
        // Header totals are exact (from the producer's raw-point pass); the profile
        // + max grade come from the stored geometry, as E2 renders them.
        let geometry = RouteStats.compute(from: decoded.points)
        let summary = RouteSummary(
            id: id,
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
            op: .upload, type: .route, objectID: route.targetObjectID ?? TransferControl.newObjectID,
            totalLen: UInt32(route.payload.count), crc32: CRC32.checksum(route.payload)
        )
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let outcome = AsyncPromise<TransferOutcome>()
        let assignedID = AsyncPromise<UInt16?>()
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

    public func downloadRides(_ ids: [RideID]) -> RideDownload {
        // Real path (A7): one download per ride object, persisted ride-by-ride, so
        // a drop keeps what landed and "resume" re-requests only the missing rides
        // (whole rides are the batch's elementary unit — spec §1 principle 4).
        .finished()
    }

    // MARK: One object over the CoC (queue-confined helpers around it)

    /// Download one object (spec §4.2 op 2): take the transfer slot, write the
    /// request, await the device's announce descriptor (`total_len` + `crc32`) —
    /// a typed reject resolves it as a throw — stream the payload off the CoC
    /// verifying the whole-object CRC, then require the committed close.
    private func downloadObject(type: ObjectType, objectID: UInt16) async throws -> Data {
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
        if let index = statusWaiters.firstIndex(where: { $0.pred(message) }) {
            statusWaiters.remove(at: index).cont.resume(returning: message)
            return
        }
        switch message {
        case .transferResult, .commandResult:
            pendingStatuses.append(message)
        case .storeChanged, .unknown:
            break  // unsolicited signals are not buffered
        }
    }

    private func rejectError(_ status: TransferResult.Status) -> DeviceError {
        status == .crcMismatch ? .crcMismatch : .transferRejected
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
                if pendingAnnounces.isEmpty {
                    announceWaiter = cont
                } else {
                    cont.resume(returning: pendingAnnounces.removeFirst())
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
            failConnect(.bluetoothUnavailable(.poweredOff))
        case .unauthorized:
            failConnect(.bluetoothUnavailable(.unauthorized))
        case .unsupported:
            failConnect(.bluetoothUnavailable(.unsupported))
        default:
            break  // .resetting / .unknown → wait for the next state update
        }
    }

    private func failConnect(_ error: DeviceError) {
        wantsConnect = false
        stateMulticast.send(.disconnected)
        connectContinuation?.resume(throwing: error)
        connectContinuation = nil
    }

    private func finishConnect() {
        stateMulticast.send(.connected)
        connectContinuation?.resume()
        connectContinuation = nil
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
    private let assignedID: AsyncPromise<UInt16?>
    private var attempt: Task<Void, Never>?

    init(
        transport: BLETransport, payload: Data, descriptor: TransferControl,
        progress: AsyncStream<TransferProgress>.Continuation,
        outcome: AsyncPromise<TransferOutcome>, assignedID: AsyncPromise<UInt16?>
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
        peripheral.discoverServices([GATT.deviceInformation, GATT.battery, GATT.obcControlService])
    }

    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        if connectContinuation != nil {
            failConnect(.notConnected)
        } else if wantsConnect {
            // A background reconnect attempt failed — keep trying; the request
            // sits pending in the controller until the device reappears.
            central.connect(peripheral)
        }
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        characteristics.removeAll()
        byteChannel = nil
        bleChannel = nil
        failAllPending()
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
            failConnect(.notConnected)
            return
        }
        for service in peripheral.services ?? [] {
            peripheral.discoverCharacteristics(nil, for: service)
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        for characteristic in service.characteristics ?? [] {
            characteristics[characteristic.uuid] = characteristic
            // Device → app notifications: battery, the `status` envelope, and the
            // download-announce descriptor on `transferControl`.
            if [GATT.batteryLevel, GATT.status, GATT.transferControl].contains(characteristic.uuid) {
                peripheral.setNotifyValue(true, for: characteristic)
            }
        }
        // Once the PSM characteristic is known, open the CoC.
        if let psm = characteristics[GATT.psm], bleChannel == nil, !openingChannel {
            openingChannel = true
            peripheral.readValue(for: psm)
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
                peripheral.openL2CAPChannel(CBL2CAPPSM(psm))
            } else {
                // A failed PSM read must not strand the open flow — fail the
                // waiters; the next transfer retries the whole open.
                openingChannel = false
                let waiters = channelWaiters
                channelWaiters.removeAll()
                for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
            }
            return
        }
        // Typed device → app `status` messages (transferResult / storeChanged / …).
        if uuid == GATT.status {
            if let data = characteristic.value, let message = try? StatusMessage(decoding: data) { deliverStatus(message) }
            return
        }
        // A notification on `transferControl` is a download-announce (our own writes
        // ack via didWriteValueFor, not here).
        if uuid == GATT.transferControl {
            if let data = characteristic.value, let descriptor = try? TransferControl(decoding: data) {
                deliverAnnounce(descriptor)
            }
            return
        }

        // Resolve a pending read.
        resumeReads(uuid, error == nil ? .success(characteristic.value ?? Data()) : .failure(DeviceError.readFailed))
    }

    public func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        let result: Result<Void, Error> = error == nil ? .success(()) : .failure(DeviceError.writeFailed)
        let conts = pendingWrites.removeValue(forKey: characteristic.uuid) ?? []
        for cont in conts { cont.resume(with: result) }
    }

    public func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        openingChannel = false
        guard let channel, error == nil else {
            let waiters = channelWaiters
            channelWaiters.removeAll()
            for cont in waiters { cont.resume(throwing: DeviceError.channelOpenFailed) }
            if connectContinuation != nil { failConnect(.channelOpenFailed) }
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
        // publishes .connected either way and resolves the initial connect's
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
