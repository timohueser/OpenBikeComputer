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
/// echo loopback and the A6 route object plane (upload / list / detail / delete / abort). It scans
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
    // Resolved by the disconnect delegate callback when the harness *induced* the drop (`disconnect()`)
    // — the A9 drop/restart + storm scenarios wait on it so a reconnect can't race the teardown.
    private var disconnectCont: CheckedContinuation<Void, Never>?

    // Device → app `status` messages, buffered so a waiter that registers just after a notification
    // arrives still sees it (no ordering race with the `transferControl`/`command` write that
    // provokes it). Waiters are predicate-matched (transferResult vs storeChanged vs commandResult
    // vs downloadAnnounce — in v2 the download announce rides `status` too as `msg = 4` (§4.3), so
    // every device → app control message shares this one buffer and ordering domain).
    private var pendingStatuses: [StatusMessage] = []
    private var statusWaiters: [(pred: @Sendable (StatusMessage) -> Bool, cont: CheckedContinuation<StatusMessage, Never>)] = []

    override init() {
        super.init()
        _ = central // force manager creation (and the first state callback)
    }

    /// Scan → connect → discover → open the CoC. Resolves when the link is ready. Re-entrant: a second
    /// call after a `disconnect()` (or a device-side drop) re-scans and brings a **fresh** `EchoLink`
    /// up — the reconnect the A9 drop/restart, storm, and back-to-back scenarios lean on. The bonded
    /// keys live in the OS keychain, so a reconnect re-encrypts with no pairing dialog.
    func connect() async throws -> EchoLink {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<EchoLink, Error>) in
            queue.async { [self] in
                resetLinkState()  // drop any stale per-link state so the reconnect starts clean
                readyCont = cont
                if central.state == .poweredOn { startScan() }
                // else: wait for centralManagerDidUpdateState → .poweredOn.
            }
        }
    }

    /// Drop the link on purpose — the A9 fault injector. Used to kill a transfer at a randomized point
    /// (the device must discard the partial and restart, spec §1 principle 4) and to cycle a
    /// connect/disconnect storm. Resolves once CoreBluetooth confirms the disconnect, so the caller can
    /// reconnect without racing the teardown. A no-op when not connected.
    ///
    /// The caller must not have a `next*()` result outstanding when it drops (the scenarios induce drops
    /// only between awaited steps) — the CoC byte reads are `withTimeout`-bounded instead, so a read
    /// left hanging by the drop surfaces as a timeout, never a leaked continuation.
    func disconnect() async {
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                guard let p = peripheral, p.state == .connected || p.state == .connecting else {
                    cont.resume()
                    return
                }
                disconnectCont = cont
                central.cancelPeripheralConnection(p)
            }
        }
    }

    /// Drop all per-link state so the next `connect()` starts clean (queue-confined). The buffered
    /// `status` messages are dropped; the status waiter queue is empty by construction at a reconnect
    /// point (the scenarios never induce a drop with a result outstanding), so there is nothing to orphan.
    private func resetLinkState() {
        openedChannel = false
        peripheral = nil
        characteristics.removeAll()
        pendingStatuses.removeAll()
    }

    /// Write the 12-byte `TransferControl` descriptor that opens/aborts a transfer (S0 §4.2). Write-only
    /// in v2 — the device answers on `status` (the download announce as `msg = 4`), not by notifying here.
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

    /// The device's next download-announce descriptor (S0 §4.3 `msg = 4`) — the same 12 bytes as the
    /// request with `total_len`/`crc32` filled in, sent before the CoC bytes flow. In v2 it rides the
    /// `status` characteristic (not `transferControl`), so it comes through the shared status buffer.
    func nextAnnounce() async -> TransferControl {
        guard case .downloadAnnounce(let descriptor) = await nextStatus(
            where: { if case .downloadAnnounce = $0 { true } else { false } })
        else { fatalError("predicate guarantees a downloadAnnounce") }
        return descriptor
    }

    // MARK: queue-confined helpers

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
        // An induced `disconnect()` is waiting on this — resolve it so the scenario can reconnect.
        if let cont = disconnectCont {
            disconnectCont = nil
            cont.resume()
        }
        // A drop *during bring-up* fails the pending `connect()`.
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
            // Device → app notifications: the `status` envelope is the sole channel in v2 — it carries
            // transferResult / storeChanged / commandResult *and* the download announce (`msg = 4`).
            // `transferControl` is write-only now (no CCCD), so it is not subscribed.
            if characteristic.uuid == GATT.status {
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
            print("echo-harness: opening L2CAP CoC on PSM \(psm)…")
            peripheral.openL2CAPChannel(CBL2CAPPSM(psm))
        case GATT.status:
            // Every device → app control message (incl. the download announce, `msg = 4`) arrives here.
            if let data = characteristic.value, let msg = try? StatusMessage(decoding: data) { deliverStatus(msg) }
        default:
            break  // v2 has no other device → app read/notify the harness consumes
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        guard let channel, error == nil, let control = characteristics[GATT.transferControl] else {
            fail(HarnessError.channelOpenFailed)
            return
        }
        let bleChannel = BLEChannel(channel: L2CAPByteChannel(channel: channel))
        readyCont?.resume(returning: EchoLink(peripheral: peripheral, transferControl: control, channel: bleChannel))
        readyCont = nil
    }
}

enum HarnessError: Error, CustomStringConvertible {
    case bluetoothUnavailable(CBManagerState)
    case connectFailed
    case disconnected
    case channelOpenFailed
    case unexpectedStatus(TransferResult.Status)
    case unexpectedCommandStatus(CommandResult.Status)
    case notByteIdentical
    case routeNotListed
    case timedOut
    case badDiagnostics
    /// A scenario invariant broke — both a harness-side check and a device-ledger (diagnostics)
    /// disagreement funnel here with a human-readable reason.
    case assertion(String)

    var description: String {
        switch self {
        case .bluetoothUnavailable(let s): return "Bluetooth unavailable (state \(s.rawValue))"
        case .connectFailed: return "failed to connect"
        case .disconnected: return "disconnected during bring-up"
        case .channelOpenFailed: return "L2CAP CoC failed to open"
        case .unexpectedStatus(let s): return "unexpected device transfer status \(s)"
        case .unexpectedCommandStatus(let s): return "unexpected device command status \(s)"
        case .notByteIdentical: return "downloaded bytes are not identical to the reference"
        case .routeNotListed: return "the route is not in the device's routeList"
        case .timedOut: return "timed out (no CoC bytes flowed — channel likely closed)"
        case .badDiagnostics: return "the diagnostics blob was not valid UTF-8"
        case .assertion(let why): return why
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
