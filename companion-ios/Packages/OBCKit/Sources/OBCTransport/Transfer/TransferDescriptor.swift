import Foundation
import OBCDomain

/// The kind of object a bulk transfer carries (`obc-ble-interface-spec.md` §4.1).
/// `config` is reserved on the CoC (the Config object crosses GATT whole-blob);
/// `firmware` is **reserved** for a future OTA type; `echo` is the A5 dev/test
/// loopback.
public enum ObjectType: UInt8, Equatable, Sendable, CaseIterable {
    case route = 1
    case ride = 2
    case configBlob = 3  // reserved on the CoC — Config crosses GATT (§3.3)
    case diagnostics = 4
    case firmware = 5  // reserved (OTA)
    case routeList = 6
    case rideList = 7
    case echo = 8  // dev/test loopback (A5)
    case trip = 9  // TR1 (#650): the trip metadata object (spec §7.7)
    case tripList = 10  // TR1 (#650): the device's trip catalog (spec §7.4)
}

/// **Control-plane** descriptor written to the `transferControl` characteristic —
/// one fixed 12-byte shape serves upload, download request, and abort, so the CoC
/// itself stays **raw payload bytes** with no per-chunk headers for the MCU to
/// parse or buffer.
///
/// Fixed **12 bytes**, little-endian (spec §4.2, v2):
/// ```
///   op         u8    1 = upload · 2 = download · 3 = abort
///   type       u8    ObjectType
///   objectID   u16   0xFFFF on upload = "new" (device assigns; see TransferResult)
///   totalLen   u32   upload: full object size · download request / abort: 0
///   crc32      u32   upload: whole-object CRC-32/IEEE · download request / abort: 0
/// ```
/// (v2 dropped v1's trailing `offset u32` — it was always 0; transfers restart,
/// not resume.) For a **download** the device answers with these same 12 bytes
/// wrapped in a `status` **`downloadAnnounce`** message (`msg = 4`), `totalLen` /
/// `crc32` filled in, before the payload flows — the announce rides `status`, not
/// `transferControl`, so all device → app control traffic shares one notify CCCD.
public struct TransferControl: Equatable, Sendable {
    public enum Op: UInt8, Equatable, Sendable, CaseIterable {
        case upload = 1
        case download = 2
        case abort = 3
    }

    /// The `objectID` an upload sends to mean "new — device assigns the id".
    public static let newObjectID: UInt16 = 0xFFFF

    public var op: Op
    public var type: ObjectType
    public var objectID: UInt16
    public var totalLen: UInt32
    public var crc32: UInt32

    public static let encodedLength = 12

    public init(op: Op, type: ObjectType, objectID: UInt16, totalLen: UInt32 = 0, crc32: UInt32 = 0) {
        self.op = op
        self.type = type
        self.objectID = objectID
        self.totalLen = totalLen
        self.crc32 = crc32
    }

    public func encode() -> Data {
        var data = Data(capacity: Self.encodedLength)
        data.append(op.rawValue)
        data.append(type.rawValue)
        data.appendLE(objectID)
        data.appendLE(totalLen)
        data.appendLE(crc32)
        return data
    }

    public init(decoding data: Data) throws {
        guard data.count >= Self.encodedLength else { throw DescriptorError.truncated }
        let b = data.startIndex
        guard let op = Op(rawValue: data[b]) else { throw DescriptorError.unknownOp(data[b]) }
        guard let type = ObjectType(rawValue: data[b + 1]) else { throw DescriptorError.unknownType(data[b + 1]) }
        self.init(
            op: op,
            type: type,
            objectID: data.readLE(at: b + 2),
            totalLen: data.readLE(at: b + 4),
            crc32: data.readLE(at: b + 8)
        )
    }
}

/// **Control-plane** result the device notifies inside the `status` envelope
/// (`StatusMessage.transferResult`) at the end of a transfer. `committedOffset` is
/// the durable byte count: the resume anchor a dropped transfer restarts from. For
/// a fresh upload the result carries the **assigned** id.
///
/// Body: `objectID u16 · status u8 · committedOffset u32` (spec §4.3). On the
/// wire the "no object" sentinel is `0xFFFF` (a fresh upload's echo when nothing
/// was committed); the codec maps it to a `nil` `objectID` here so consumers
/// never compare against the raw sentinel.
public struct TransferResult: Equatable, Sendable {
    /// Terminal transfer status (spec §4.3). An **unknown** code is *not* a member
    /// here — the decoder maps it to `.error` (generic device failure) rather than
    /// throwing, so a future reject the app doesn't recognize can't wedge a
    /// transfer or fail the link (forward compat).
    public enum Status: UInt8, Equatable, Sendable, CaseIterable {
        case committed = 0    // stored + CRC verified
        case crcMismatch = 1  // rejected, not committed
        case aborted = 2      // canceled by either side
        case error = 3        // storage / internal failure
        case notFound = 4     // unknown object type/id
        case busy = 5         // a transfer is already active
        case storageFull = 6  // new-route upload rejected at open — catalog full
    }

    public var objectID: DeviceObjectID?
    public var status: Status
    public var committedOffset: UInt32

    public init(objectID: DeviceObjectID?, status: Status, committedOffset: UInt32) {
        self.objectID = objectID
        self.status = status
        self.committedOffset = committedOffset
    }
}

// MARK: - Exchange correlation

extension TransferControl {
    /// Whether a device download announce answers this exact request. The v2
    /// status stream has no transaction id, so type + object id are the
    /// correlation key; accepting a different announce would consume raw bytes
    /// under the wrong length/CRC and desynchronise the shared CoC.
    func acceptsDownloadAnnounce(_ announce: TransferControl) -> Bool {
        op == .download
            && announce.op == .download
            && announce.type == type
            && announce.objectID == objectID
    }

    /// Whether a committed close can belong to this exchange. In addition to
    /// the object id, the durable byte count is an inexpensive transaction
    /// fingerprint: it rejects a late result from the preceding transfer.
    func acceptsCommittedResult(_ result: TransferResult, byteCount: UInt32) -> Bool {
        guard result.status == .committed,
              result.committedOffset == byteCount,
              let resultID = result.objectID else { return false }
        if op == .download { return resultID.raw == objectID }
        if op == .upload, objectID != Self.newObjectID { return resultID.raw == objectID }
        return op == .upload
    }
}

/// A change signal on the `storeChanged` status message (spec §4.3): which object
/// store moved (route/ride/trip) and the new revision.
public struct StoreChanged: Equatable, Sendable {
    public var type: ObjectType
    public var revision: UInt32

    public init(type: ObjectType, revision: UInt32) {
        self.type = type
        self.revision = revision
    }
}

/// The result of a `command` write, notified on `status` (spec §4.3/§4.4).
public struct CommandResult: Equatable, Sendable {
    public enum Status: UInt8, Equatable, Sendable, CaseIterable {
        case ok = 0
        case unknownCommand = 1
        case notFound = 2
        case busy = 3
        case error = 4
    }

    public var command: UInt8
    public var status: Status
    public var detail: UInt8

    public init(command: UInt8, status: Status, detail: UInt8 = 0) {
        self.command = command
        self.status = status
        self.detail = detail
    }
}

/// One `status` characteristic notification: a `u8` discriminator + fixed body
/// (spec §4.3). Unknown discriminators decode to `.unknown` — the app must ignore
/// them (forward compatibility), never fail the link over one.
public enum StatusMessage: Equatable, Sendable {
    case transferResult(TransferResult)  // msg = 1, 8 bytes total
    case storeChanged(StoreChanged)      // msg = 2, 6 bytes total
    case commandResult(CommandResult)    // msg = 3, 4 bytes total
    case downloadAnnounce(TransferControl)  // msg = 4, 13 bytes total (msg + 12-byte descriptor)
    case unknown(UInt8)                  // forward-compatible: ignore

    public func encode() -> Data {
        var data = Data()
        switch self {
        case .transferResult(let r):
            data.append(1)
            data.appendLE(r.objectID?.raw ?? TransferControl.newObjectID)
            data.append(r.status.rawValue)
            data.appendLE(r.committedOffset)
        case .storeChanged(let s):
            data.append(2)
            data.append(s.type.rawValue)
            data.appendLE(s.revision)
        case .commandResult(let c):
            data.append(3)
            data.append(c.command)
            data.append(c.status.rawValue)
            data.append(c.detail)
        case .downloadAnnounce(let descriptor):
            data.append(4)
            data.append(descriptor.encode())
        case .unknown(let msg):
            data.append(msg)
        }
        return data
    }

    public init(decoding data: Data) throws {
        guard let msg = data.first else { throw DescriptorError.truncated }
        let b = data.startIndex
        switch msg {
        case 1:
            guard data.count >= 8 else { throw DescriptorError.truncated }
            // Forward compat (spec §4.3): an unrecognized transfer status is not a
            // decode failure — it's a generic device reject. Fall back to `.error`
            // so the result still delivers and the transfer resolves as a failure,
            // rather than throwing (which would silently drop the message and wedge
            // the transfer, since the notification handler decodes with `try?`).
            let status = TransferResult.Status(rawValue: data[b + 3]) ?? .error
            let rawID: UInt16 = data.readLE(at: b + 1)
            self = .transferResult(TransferResult(
                objectID: rawID == TransferControl.newObjectID ? nil : DeviceObjectID(rawID),
                status: status, committedOffset: data.readLE(at: b + 4)
            ))
        case 2:
            guard data.count >= 6 else { throw DescriptorError.truncated }
            guard let type = ObjectType(rawValue: data[b + 1]) else { throw DescriptorError.unknownType(data[b + 1]) }
            self = .storeChanged(StoreChanged(type: type, revision: data.readLE(at: b + 2)))
        case 3:
            guard data.count >= 4 else { throw DescriptorError.truncated }
            guard let status = CommandResult.Status(rawValue: data[b + 2]) else {
                throw DescriptorError.unknownStatus(data[b + 2])
            }
            self = .commandResult(CommandResult(command: data[b + 1], status: status, detail: data[b + 3]))
        case 4:
            // downloadAnnounce (spec §4.3): the msg byte + the 12-byte transfer
            // descriptor. A short body under a *known* discriminator is a decode
            // error (unlike an unknown discriminator, which is ignored below) —
            // `TransferControl(decoding:)` throws `.truncated` when the descriptor
            // slice is under 12 bytes, and `.unknownOp` / `.unknownType` for a
            // malformed descriptor.
            self = .downloadAnnounce(try TransferControl(decoding: data[(b + 1)...]))
        default:
            self = .unknown(msg)
        }
    }
}

/// Why a control-plane descriptor failed to decode.
public enum DescriptorError: Error, Equatable, Sendable {
    case truncated
    case unknownOp(UInt8)
    case unknownType(UInt8)
    case unknownStatus(UInt8)
}

// MARK: - Little-endian (de)serialization

extension Data {
    fileprivate mutating func appendLE(_ value: UInt16) {
        append(UInt8(value & 0xFF)); append(UInt8((value >> 8) & 0xFF))
    }

    fileprivate mutating func appendLE(_ value: UInt32) {
        append(UInt8(value & 0xFF)); append(UInt8((value >> 8) & 0xFF))
        append(UInt8((value >> 16) & 0xFF)); append(UInt8((value >> 24) & 0xFF))
    }

    fileprivate func readLE(at index: Index) -> UInt16 {
        UInt16(self[index]) | (UInt16(self[index + 1]) << 8)
    }

    fileprivate func readLE(at index: Index) -> UInt32 {
        UInt32(self[index]) | (UInt32(self[index + 1]) << 8)
            | (UInt32(self[index + 2]) << 16) | (UInt32(self[index + 3]) << 24)
    }
}
