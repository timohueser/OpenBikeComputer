#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCTransport

/// A ready link: the control-plane characteristics + the opened CoC byte layer, all reusing
/// `OBCTransport`'s real transport code (`GATT`, `BLEChannel`, `L2CAPByteChannel`).
struct EchoLink: @unchecked Sendable {
    let peripheral: CBPeripheral
    let transferControl: CBCharacteristic
    /// The raw CoC byte layer — the same `BLEChannel` the iOS app streams objects over.
    let channel: BLEChannel
}

/// A minimal CoreBluetooth central that brings up an OBC link and drives both data planes: the A5
/// echo loopback and the A6 route object plane (upload / list / detail / delete / resume). It scans
/// for the OBC Control service, connects, discovers, reads the `psm`, and opens the L2CAP CoC. It
/// owns its *own* `CBCentralManager` (the app's `BLETransport` wraps the same steps behind the
/// semantic `DeviceTransport`, which has no harness verbs) but reuses the pinned `GATT` UUIDs, the
/// `L2CAPByteChannel`/`BLEChannel` byte plane, and the `TransferControl`/`StatusMessage`/`RouteList`
/// codecs — so the bytes on the wire are exactly the app's.
///
/// All mutable state is confined to the CoreBluetooth callback `queue`; async methods hop onto it
/// and register continuations the delegate callbacks resolve — the same confinement pattern as
/// `BLETransport`, which is why this is a plain `@unchecked Sendable` class.
final class EchoCentral: NSObject, @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.openbikecomputer.echo-harness")
    private lazy var central = CBCentralManager(delegate: self, queue: queue)

    private var peripheral: CBPeripheral?
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    private var readyCont: CheckedContinuation<EchoLink, Error>?
    private var openedChannel = false
    /// The PSM the device published, kept so a resume can re-open the CoC without re-reading it.
    private var lastPSM: UInt16 = 0
    /// The live CoC byte layer — retained so the harness can close it (induce a drop) and re-open a
    /// fresh one for an offset-resume.
    private var currentByteChannel: L2CAPByteChannel?
    private var channelReopenCont: CheckedContinuation<BLEChannel, Error>?

    // Device → app `status` messages, buffered so a waiter that registers just after a notification
    // arrives still sees it (no ordering race with the `transferControl`/`command` write that
    // provokes it). Waiters are predicate-matched (transferResult vs storeChanged vs commandResult).
    private var pendingStatuses: [StatusMessage] = []
    private var statusWaiters: [(pred: @Sendable (StatusMessage) -> Bool, cont: CheckedContinuation<StatusMessage, Never>)] = []

    // Download-announce notifications on the `transferControl` characteristic (the device fills in
    // total_len + crc32 before it streams), same buffering discipline.
    private var pendingAnnounces: [TransferControl] = []
    private var announceWaiter: CheckedContinuation<TransferControl, Never>?

    // In-flight GATT reads (the digest), keyed by characteristic.
    private var pendingReads: [CBUUID: [CheckedContinuation<Data, Error>]] = [:]

    override init() {
        super.init()
        _ = central // force manager creation (and the first state callback)
    }

    /// Scan → connect → discover → open the CoC. Resolves when the link is ready.
    func connect() async throws -> EchoLink {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<EchoLink, Error>) in
            queue.async { [self] in
                readyCont = cont
                if central.state == .poweredOn { startScan() }
                // else: wait for centralManagerDidUpdateState → .poweredOn.
            }
        }
    }

    /// Write the 16-byte `TransferControl` descriptor that opens/resumes/aborts a transfer (S0 §4.2).
    func writeControl(_ bytes: Data, to characteristic: CBCharacteristic) {
        queue.async { [self] in peripheral?.writeValue(bytes, for: characteristic, type: .withResponse) }
    }

    /// Write a `command` imperative (S0 §4.4) — e.g. `deleteObject`.
    func writeCommand(_ bytes: Data) {
        queue.async { [self] in
            if let c = characteristics[GATT.command] { peripheral?.writeValue(bytes, for: c, type: .withResponse) }
        }
    }

    /// The device's next `transferResult` (S0 §4.3) — a transfer's committed/aborted/… verdict.
    func nextTransferResult() async -> TransferResult {
        guard case .transferResult(let r) = await nextStatus(where: { if case .transferResult = $0 { true } else { false } })
        else { fatalError("predicate guarantees a transferResult") }
        return r
    }

    /// The device's next `commandResult` (S0 §4.3/§4.4).
    func nextCommandResult() async -> CommandResult {
        guard case .commandResult(let c) = await nextStatus(where: { if case .commandResult = $0 { true } else { false } })
        else { fatalError("predicate guarantees a commandResult") }
        return c
    }

    /// The device's next `storeChanged` (S0 §4.3) — emitted after every commit/delete.
    func nextStoreChanged() async -> StoreChanged {
        guard case .storeChanged(let s) = await nextStatus(where: { if case .storeChanged = $0 { true } else { false } })
        else { fatalError("predicate guarantees a storeChanged") }
        return s
    }

    /// The device's next download-announce descriptor on `transferControl` (S0 §4.2) — the same 16
    /// bytes as the request with `total_len`/`crc32` filled in, sent before the CoC bytes flow.
    func nextAnnounce() async -> TransferControl {
        await withCheckedContinuation { (cont: CheckedContinuation<TransferControl, Never>) in
            queue.async { [self] in
                if pendingAnnounces.isEmpty { announceWaiter = cont }
                else { cont.resume(returning: pendingAnnounces.removeFirst()) }
            }
        }
    }

    /// Read the `objectStore` digest (S0 §4.5): revision + object counts.
    func readDigest() async throws -> ObjectStoreDigest {
        try ObjectStoreDigest(decoding: try await readValue(GATT.objectStore))
    }

    /// Close the live CoC — the harness's way to induce a mid-transfer drop (the ACL stays up).
    func closeChannel() async {
        let channel: L2CAPByteChannel? = await withCheckedContinuation { cont in
            queue.async { [self] in cont.resume(returning: currentByteChannel) }
        }
        await channel?.close()
    }

    /// Re-open the CoC on the published PSM and return a fresh `BLEChannel` — the offset-resume path
    /// (S0 §4.2: "the app re-opens the CoC and resumes by offset").
    func reopenChannel() async throws -> BLEChannel {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<BLEChannel, Error>) in
            queue.async { [self] in
                channelReopenCont = cont
                peripheral?.openL2CAPChannel(CBL2CAPPSM(lastPSM))
            }
        }
    }

    // MARK: queue-confined helpers

    private func readValue(_ uuid: CBUUID) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            queue.async { [self] in
                guard let peripheral, let characteristic = characteristics[uuid] else {
                    cont.resume(throwing: HarnessError.characteristicMissing)
                    return
                }
                pendingReads[uuid, default: []].append(cont)
                peripheral.readValue(for: characteristic)
            }
        }
    }

    private func nextStatus(where pred: @escaping @Sendable (StatusMessage) -> Bool) async -> StatusMessage {
        await withCheckedContinuation { (cont: CheckedContinuation<StatusMessage, Never>) in
            queue.async { [self] in
                if let i = pendingStatuses.firstIndex(where: pred) {
                    cont.resume(returning: pendingStatuses.remove(at: i))
                } else {
                    statusWaiters.append((pred, cont))
                }
            }
        }
    }

    private func startScan() {
        print("echo-harness: scanning for \(GATT.obcControlService)…")
        central.scanForPeripherals(withServices: [GATT.obcControlService])
    }

    private func fail(_ error: Error) {
        readyCont?.resume(throwing: error)
        readyCont = nil
    }

    private func deliverStatus(_ msg: StatusMessage) {
        if let i = statusWaiters.firstIndex(where: { $0.pred(msg) }) {
            statusWaiters.remove(at: i).cont.resume(returning: msg)
        } else {
            pendingStatuses.append(msg)
        }
    }

    private func deliverAnnounce(_ desc: TransferControl) {
        if let waiter = announceWaiter {
            announceWaiter = nil
            waiter.resume(returning: desc)
        } else {
            pendingAnnounces.append(desc)
        }
    }
}

// MARK: - CBCentralManagerDelegate

extension EchoCentral: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn where readyCont != nil: startScan()
        case .poweredOff, .unauthorized, .unsupported: fail(HarnessError.bluetoothUnavailable(central.state))
        default: break
        }
    }

    func centralManager(
        _ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any], rssi RSSI: NSNumber
    ) {
        central.stopScan()
        self.peripheral = peripheral
        peripheral.delegate = self
        print("echo-harness: found \(peripheral.name ?? "OBC") — connecting…")
        central.connect(peripheral)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        peripheral.discoverServices([GATT.obcControlService])
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        fail(HarnessError.connectFailed)
    }

    func centralManager(
        _ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?
    ) {
        if readyCont != nil { fail(HarnessError.disconnected) }
    }
}

// MARK: - CBPeripheralDelegate

extension EchoCentral: CBPeripheralDelegate {
    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        for service in peripheral.services ?? [] {
            peripheral.discoverCharacteristics(nil, for: service)
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        for characteristic in service.characteristics ?? [] {
            characteristics[characteristic.uuid] = characteristic
            // Device → app notifications: the `status` envelope and the download-announce descriptor.
            if characteristic.uuid == GATT.status || characteristic.uuid == GATT.transferControl {
                peripheral.setNotifyValue(true, for: characteristic)
            }
        }
        // Once the PSM is known, read it and open the CoC.
        if let psm = characteristics[GATT.psm], !openedChannel {
            peripheral.readValue(for: psm)
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        switch characteristic.uuid {
        case GATT.psm where !openedChannel:
            guard let data = characteristic.value, data.count >= 2 else { return }
            openedChannel = true
            let psm = UInt16(data[data.startIndex]) | (UInt16(data[data.startIndex + 1]) << 8)
            lastPSM = psm
            print("echo-harness: opening L2CAP CoC on PSM \(psm)…")
            peripheral.openL2CAPChannel(CBL2CAPPSM(psm))
        case GATT.status:
            if let data = characteristic.value, let msg = try? StatusMessage(decoding: data) { deliverStatus(msg) }
        case GATT.transferControl:
            // A device → app notification here is a download-announce (our own writes don't echo).
            if let data = characteristic.value, let desc = try? TransferControl(decoding: data) { deliverAnnounce(desc) }
        default:
            // A completed GATT read (the digest) — resume the oldest waiter for this characteristic.
            guard var waiters = pendingReads[characteristic.uuid], !waiters.isEmpty else { return }
            let cont = waiters.removeFirst()
            pendingReads[characteristic.uuid] = waiters
            if let error { cont.resume(throwing: error) } else { cont.resume(returning: characteristic.value ?? Data()) }
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        guard let channel, error == nil else {
            if let cont = channelReopenCont { channelReopenCont = nil; cont.resume(throwing: HarnessError.channelOpenFailed) }
            else { fail(HarnessError.channelOpenFailed) }
            return
        }
        let byteChannel = L2CAPByteChannel(channel: channel)
        currentByteChannel = byteChannel
        let bleChannel = BLEChannel(channel: byteChannel)
        if let cont = channelReopenCont {
            channelReopenCont = nil
            cont.resume(returning: bleChannel)
        } else if let control = characteristics[GATT.transferControl] {
            readyCont?.resume(returning: EchoLink(peripheral: peripheral, transferControl: control, channel: bleChannel))
            readyCont = nil
        } else {
            fail(HarnessError.channelOpenFailed)
        }
    }
}

enum HarnessError: Error, CustomStringConvertible {
    case bluetoothUnavailable(CBManagerState)
    case connectFailed
    case disconnected
    case channelOpenFailed
    case characteristicMissing
    case unexpectedStatus(TransferResult.Status)
    case unexpectedCommandStatus(CommandResult.Status)
    case notByteIdentical
    case digestUnchanged
    case routeNotListed
    case timedOut

    var description: String {
        switch self {
        case .bluetoothUnavailable(let s): return "Bluetooth unavailable (state \(s.rawValue))"
        case .connectFailed: return "failed to connect"
        case .disconnected: return "disconnected during bring-up"
        case .channelOpenFailed: return "L2CAP CoC failed to open"
        case .characteristicMissing: return "a required characteristic wasn't discovered"
        case .unexpectedStatus(let s): return "unexpected device transfer status \(s)"
        case .unexpectedCommandStatus(let s): return "unexpected device command status \(s)"
        case .notByteIdentical: return "downloaded bytes are not identical to the reference"
        case .digestUnchanged: return "the store digest revision did not change"
        case .routeNotListed: return "the route is not in the device's routeList"
        case .timedOut: return "timed out (no CoC bytes flowed — channel likely closed)"
        }
    }
}

/// Race `op` against a deadline so a stalled CoC reports instead of hanging the whole run.
func withTimeout<T: Sendable>(_ seconds: Double, _ op: @escaping @Sendable () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await op() }
        group.addTask {
            try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            throw HarnessError.timedOut
        }
        defer { group.cancelAll() }
        return try await group.next()!
    }
}
#endif
