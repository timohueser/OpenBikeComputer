#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCDomain

/// The **real** `DeviceTransport` (Tier 1 → CoreBluetooth). Scans for the OBC
/// service, connects, discovers DIS/BAS/OBC Control, reads the PSM and opens the
/// L2CAP CoC, and maps the semantic protocol onto GATT reads/writes/notifies +
/// the `BLEChannel` byte layer.
///
/// > **Real path gated on firmware `A4`/`A5` (+ `A8` for bonding).** The connection
/// > lifecycle and the SIG-standard reads (DIS/BAS) are wired for bring-up but are
/// > **not yet hardware-validated** — no device advertises the service or CoC PSM
/// > until Track-A lands. The custom UUIDs and payload layouts are **pinned by
/// > firmware S0** (`obc-ble-interface-spec.md`); the framing/codec layer beneath
/// > (`BLEChannel`, the descriptors, the codecs) is fully host-tested, including
/// > against the shared `protocol-vectors/` fixtures.
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
    private var bleChannel: BLEChannel?

    // Outstanding operations (all touched only on `queue`).
    private var connectContinuation: CheckedContinuation<Void, Error>?
    private var wantsConnect = false
    private var pendingReads: [CBUUID: [CheckedContinuation<Data, Error>]] = [:]
    private var pendingWrites: [CBUUID: [CheckedContinuation<Void, Error>]] = [:]

    // Device → app notifications, buffered so a waiter that registers just after a
    // notification arrives still sees it (no race with the write that provokes it) —
    // the same discipline the EchoHarness uses to drive this flow on glass. Cleared
    // on disconnect; a hung in-flight download surfaces as the dropped CoC channel.
    private var pendingStatuses: [StatusMessage] = []
    private var statusWaiters: [(pred: @Sendable (StatusMessage) -> Bool, cont: CheckedContinuation<StatusMessage, Never>)] = []
    private var pendingAnnounces: [TransferControl] = []
    private var announceWaiter: CheckedContinuation<TransferControl, Never>?

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
        // Diagnostics are a CoC object (type 4, spec §7.5); the characteristic slot
        // is reserved. The object download lands with A5 bring-up.
        throw DeviceError.readFailed
    }

    public func deleteRoute(_ id: RouteID) async throws {
        // `deleteObject` (cmd 1): `cmd u8 · type u8 · object_id u16 LE` — spec §4.4.
        // The commandResult status notification is consumed at A4 bring-up.
        let objectID = UInt16(id.rawValue) ?? 0
        let payload = Data([1, ObjectType.route.rawValue, UInt8(objectID & 0xFF), UInt8(objectID >> 8)])
        try await write(payload, to: GATT.command)
    }

    public func listRoutes() async throws -> [RouteSummary] {
        // The `routeList` object (type 6, spec §7.4) over the CoC → the catalog.
        // Geometry isn't downloaded here; `trackPreview`/waypoints fill on
        // `routeDetail`.
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
        // The `rideList` object (type 7, spec §7.4) — the ride catalog. The
        // per-ride payload download + `RideObjectCodec` decode is the sync path (B7).
        _ = try await read(GATT.objectStore)
        return []
    }

    public func routeDetail(_ id: RouteID) async throws -> RouteDetail {
        // Pinned by S0 as "download the route object" (spec §7.1): the stored OBCR
        // v2 blob, decoded app-side for the waypoints + elevation profile — one
        // layout, one truth.
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
        _ = try await read(GATT.objectStore)
        throw DeviceError.readFailed
    }

    // MARK: DeviceTransport — data plane

    public func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        // A fresh upload sends objectID 0xFFFF = "new" (the device assigns one);
        // re-uploading an edited route sends its stored id, which replaces that
        // object in place (spec §4.1/§4.2). Either way the device reports the id
        // in the closing transferResult, which we surface as `assignedObjectID`.
        let start = TransferControl(
            op: .upload, type: .route, objectID: route.targetObjectID ?? TransferControl.newObjectID,
            totalLen: UInt32(route.payload.count), crc32: CRC32.checksum(route.payload)
        )
        let assignedID = AsyncPromise<UInt16?>()
        let handle = beginTransfer(start) { channel in channel.upload(route.payload, assignedObjectID: assignedID) }
        Task { [weak self] in
            guard let self, await handle.outcome == .completed else { assignedID.fulfill(nil); return }
            let result = await self.nextTransferResult()
            assignedID.fulfill(result.status == .committed ? result.objectID : nil)
        }
        return handle
    }

    public func downloadRides(_ ids: [RideID]) -> RideDownload {
        // Real path (B7/A7): per ride the app writes a download request
        // (`TransferControl` op 2), the device answers with the same descriptor
        // filled in (totalLen + CRC), the app calls
        // `channel.download(length:expectedCRC:)` and yields each verified payload
        // into `rides`. The CoC request/announce loop lands with A5 bring-up.
        .finished()
    }

    /// Announce a transfer on the control plane (`transferControl`), then stream its
    /// payload over the CoC via `make`. Returns an immediately-finished handle if not
    /// connected (caller detects the drop via `state`).
    private func beginTransfer(_ start: TransferControl, _ make: @Sendable @escaping (BLEChannel) -> TransferHandle) -> TransferHandle {
        queue.sync {
            guard let bleChannel, let peripheral, let control = characteristics[GATT.transferControl] else {
                return TransferHandle.immediatelyFinished(.failed(.notConnected))
            }
            // Open the transfer before the first CoC byte. Exact sequencing vs the
            // stream (and the Status/committedOffset handshake for resume) is
            // finalized at A5 bring-up.
            peripheral.writeValue(start.encode(), for: control, type: .withResponse)
            return make(bleChannel)
        }
    }

    /// Download one object over the CoC (spec §4.2 op 2): write the request, await
    /// the device's announce descriptor (`total_len` + `crc32`), stream the payload
    /// off the CoC verifying the whole-object CRC, then consume the closing
    /// `transferResult`. The proven EchoHarness `downloadObject` flow, behind the
    /// semantic transport. Throws if the link isn't up or the transfer doesn't commit.
    private func downloadObject(type: ObjectType, objectID: UInt16) async throws -> Data {
        let channel = try queue.sync { () -> BLEChannel in
            guard let bleChannel else { throw DeviceError.notConnected }
            return bleChannel
        }
        try await write(TransferControl(op: .download, type: type, objectID: objectID).encode(), to: GATT.transferControl)
        let announce = await nextAnnounce()
        let (_, task) = channel.download(length: Int(announce.totalLen), expectedCRC: announce.crc32)
        let bytes = try await task.value
        guard await nextTransferResult().status == .committed else { throw DeviceError.readFailed }
        return bytes
    }

    // MARK: Status / announce notifications (queue-confined)

    private func deliverStatus(_ message: StatusMessage) {
        if let index = statusWaiters.firstIndex(where: { $0.pred(message) }) {
            statusWaiters.remove(at: index).cont.resume(returning: message)
        } else {
            pendingStatuses.append(message)
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

    private func nextStatus(where predicate: @escaping @Sendable (StatusMessage) -> Bool) async -> StatusMessage {
        await withCheckedContinuation { (cont: CheckedContinuation<StatusMessage, Never>) in
            queue.async { [self] in
                if let index = pendingStatuses.firstIndex(where: predicate) {
                    cont.resume(returning: pendingStatuses.remove(at: index))
                } else {
                    statusWaiters.append((predicate, cont))
                }
            }
        }
    }

    private func nextTransferResult() async -> TransferResult {
        guard case .transferResult(let result) = await nextStatus(where: {
            if case .transferResult = $0 { true } else { false }
        }) else { fatalError("predicate guarantees a transferResult") }
        return result
    }

    private func nextAnnounce() async -> TransferControl {
        await withCheckedContinuation { (cont: CheckedContinuation<TransferControl, Never>) in
            queue.async { [self] in
                if pendingAnnounces.isEmpty { announceWaiter = cont }
                else { cont.resume(returning: pendingAnnounces.removeFirst()) }
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

    // MARK: Async ↔ delegate bridges (queue-confined)

    private func read(_ uuid: CBUUID) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: DeviceError.notConnected); return
                }
                pendingReads[uuid, default: []].append(cont)
                peripheral.readValue(for: characteristic)
            }
        }
    }

    private func readString(_ uuid: CBUUID) async throws -> String {
        String(decoding: try await read(uuid), as: UTF8.self)
    }

    private func write(_ data: Data, to uuid: CBUUID) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: DeviceError.notConnected); return
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
        failConnect(.notConnected)
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        characteristics.removeAll()
        bleChannel = nil
        // Drop any buffered notifications from the dead link (a new connection
        // re-announces); an in-flight download surfaces the drop via its CoC read.
        pendingStatuses.removeAll()
        pendingAnnounces.removeAll()
        stateMulticast.send(wantsConnect ? .outOfRange : .disconnected)
    }
}

// MARK: - CBPeripheralDelegate

extension BLETransport: CBPeripheralDelegate {
    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard error == nil else { failConnect(.notConnected); return }
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
        if characteristics[GATT.psm] != nil, bleChannel == nil {
            peripheral.readValue(for: characteristics[GATT.psm]!)
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        let uuid = characteristic.uuid

        // BAS battery notify → multicast.
        if uuid == GATT.batteryLevel, let value = characteristic.value?.first {
            batteryMulticast.send(Int(value))
            return
        }
        // PSM read → open the L2CAP channel.
        if uuid == GATT.psm, bleChannel == nil {
            if let data = characteristic.value, data.count >= 2 {
                let psm = UInt16(data[0]) | (UInt16(data[1]) << 8)
                peripheral.openL2CAPChannel(CBL2CAPPSM(psm))
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
        guard let channel, error == nil else { failConnect(.channelOpenFailed); return }
        bleChannel = BLEChannel(channel: L2CAPByteChannel(channel: channel))
        // CoC up + services discovered → the link is ready.
        finishConnect()
    }

    private func resumeReads(_ uuid: CBUUID, _ result: Result<Data, Error>) {
        let conts = pendingReads.removeValue(forKey: uuid) ?? []
        for cont in conts { cont.resume(with: result) }
    }
}
#endif
