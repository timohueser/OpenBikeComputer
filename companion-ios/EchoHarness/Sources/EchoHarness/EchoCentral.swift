#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation
import OBCTransport

/// A ready echo link: the control-plane characteristics + the opened CoC byte layer, all reusing
/// `OBCTransport`'s real transport code (`GATT`, `BLEChannel`, `L2CAPByteChannel`).
struct EchoLink: @unchecked Sendable {
    let peripheral: CBPeripheral
    let transferControl: CBCharacteristic
    /// The raw CoC byte layer — the same `BLEChannel` the iOS app streams objects over.
    let channel: BLEChannel
}

/// A minimal CoreBluetooth central that brings up an OBC link far enough to drive the A5 echo
/// loopback: scan for the OBC Control service, connect, discover, read the `psm` characteristic, and
/// open the L2CAP CoC. It deliberately owns its *own* `CBCentralManager` (the app's `BLETransport`
/// wraps the same steps behind the semantic `DeviceTransport`, which has no echo verb) but reuses
/// the pinned `GATT` UUIDs and the `L2CAPByteChannel`/`BLEChannel` byte plane, so the bytes on the
/// wire are exactly the app's.
///
/// All mutable state is confined to the CoreBluetooth callback `queue`; async methods hop onto it and
/// register continuations the delegate callbacks resolve — the same confinement pattern as
/// `BLETransport`, which is why this is a plain `@unchecked Sendable` class.
final class EchoCentral: NSObject, @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.openbikecomputer.echo-harness")
    private lazy var central = CBCentralManager(delegate: self, queue: queue)

    private var peripheral: CBPeripheral?
    private var characteristics: [CBUUID: CBCharacteristic] = [:]
    private var readyCont: CheckedContinuation<EchoLink, Error>?
    private var openedChannel = false

    // Device → app `status` results, buffered so a `nextTransferResult()` that registers just after
    // the notification arrives still sees it (no ordering race with the `transferControl` write).
    private var pendingResults: [TransferResult] = []
    private var resultWaiter: CheckedContinuation<TransferResult, Never>?

    override init() {
        super.init()
        _ = central // force manager creation (and the first state callback)
    }

    /// Scan → connect → discover → open the CoC. Resolves when the link is ready to echo.
    func connect() async throws -> EchoLink {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<EchoLink, Error>) in
            queue.async { [self] in
                readyCont = cont
                if central.state == .poweredOn { startScan() }
                // else: wait for centralManagerDidUpdateState → .poweredOn.
            }
        }
    }

    /// Write the 16-byte `TransferControl` descriptor that opens/aborts a transfer (S0 §4.2).
    func writeControl(_ bytes: Data, to characteristic: CBCharacteristic) {
        queue.async { [self] in peripheral?.writeValue(bytes, for: characteristic, type: .withResponse) }
    }

    /// The device's next `transferResult` (S0 §4.3) — the committed/crcMismatch verdict.
    func nextTransferResult() async -> TransferResult {
        await withCheckedContinuation { (cont: CheckedContinuation<TransferResult, Never>) in
            queue.async { [self] in
                if pendingResults.isEmpty {
                    resultWaiter = cont
                } else {
                    cont.resume(returning: pendingResults.removeFirst())
                }
            }
        }
    }

    // MARK: queue-confined helpers

    private func startScan() {
        print("echo-harness: scanning for \(GATT.obcControlService)…")
        central.scanForPeripherals(withServices: [GATT.obcControlService])
    }

    private func fail(_ error: Error) {
        readyCont?.resume(throwing: error)
        readyCont = nil
    }

    private func deliverResult(_ result: TransferResult) {
        if let waiter = resultWaiter {
            resultWaiter = nil
            waiter.resume(returning: result)
        } else {
            pendingResults.append(result)
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
            if characteristic.uuid == GATT.status {
                peripheral.setNotifyValue(true, for: characteristic) // device → app transfer results
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
            // Decode the `status` notification; forward a transferResult to the waiter.
            if let data = characteristic.value, case .transferResult(let r)? = try? StatusMessage(decoding: data) {
                deliverResult(r)
            }
        default:
            break
        }
    }

    func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        guard let channel, error == nil, let control = characteristics[GATT.transferControl] else {
            fail(HarnessError.channelOpenFailed)
            return
        }
        let link = EchoLink(
            peripheral: peripheral,
            transferControl: control,
            channel: BLEChannel(channel: L2CAPByteChannel(channel: channel))
        )
        readyCont?.resume(returning: link)
        readyCont = nil
    }
}

enum HarnessError: Error, CustomStringConvertible {
    case bluetoothUnavailable(CBManagerState)
    case connectFailed
    case disconnected
    case channelOpenFailed
    case unexpectedStatus(TransferResult.Status)
    case notByteIdentical
    case timedOut

    var description: String {
        switch self {
        case .bluetoothUnavailable(let s): return "Bluetooth unavailable (state \(s.rawValue))"
        case .connectFailed: return "failed to connect"
        case .disconnected: return "disconnected during bring-up"
        case .channelOpenFailed: return "L2CAP CoC failed to open"
        case .unexpectedStatus(let s): return "unexpected device status \(s)"
        case .notByteIdentical: return "echoed bytes are not identical to what was sent"
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
