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
        // Lists are CoC objects (`routeList` type 6, spec §7.4) — the digest read is
        // wired; the object download + decode land with A5 bring-up.
        _ = try await read(GATT.objectStore)
        return []
    }

    public func listRides() async throws -> [RideSummary] {
        _ = try await read(GATT.objectStore)
        return []
    }

    public func routeDetail(_ id: RouteID) async throws -> RouteDetail {
        // Pinned by S0 as "download the route object" (the stored OBCR v2 blob,
        // spec §7.1); the CoC download + OBCR decode land with A5/A6 bring-up.
        _ = try await read(GATT.objectStore)
        throw DeviceError.readFailed
    }

    public func rideDetail(_ id: RideID) async throws -> RideDetail {
        _ = try await read(GATT.objectStore)
        throw DeviceError.readFailed
    }

    // MARK: DeviceTransport — data plane

    public func uploadRoute(_ route: RouteBlob) -> TransferHandle {
        // A fresh upload sends objectID 0xFFFF = "new"; the device assigns the id
        // and reports it in the transferResult (spec §4.1/§4.3).
        let start = TransferControl(
            op: .upload, type: .route, objectID: TransferControl.newObjectID,
            totalLen: UInt32(route.payload.count), crc32: CRC32.checksum(route.payload)
        )
        return beginTransfer(start) { channel in channel.upload(route.payload) }
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
            if characteristic.uuid == GATT.batteryLevel || characteristic.uuid == GATT.status {
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
