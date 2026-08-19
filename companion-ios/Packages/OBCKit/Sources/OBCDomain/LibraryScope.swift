import Foundation

/// The device and store identity every id-keyed library fact is scoped by.
///
/// Device object ids are durable only *within* an id era: an RRAM loss (full
/// reflash, factory reset, torn id-marks line) reopens the id space, and two
/// devices mint ids into disjoint namespaces that look identical on the wire.
/// The scope pins both axes — `serial` from DIS (stable for the hardware's
/// life), `epoch` from the widened `protocolVersion` read (a TRNG nonce the
/// device re-mints only on an era reset). State keyed under one scope can
/// never collide with, suppress, or overwrite state under another **by
/// construction**: an era change needs zero migration code, because the old
/// era's keys simply stop matching (its entries become archival) and the new
/// era's sets start empty.
public struct LibraryScope: Hashable, Sendable {
    /// DIS serial-number string (0x2A25) of the device this scope belongs to.
    public let serial: String
    /// The device's store-epoch nonce at the time the scope was read.
    public let epoch: UInt32
    /// Protocol-v4's exact 128-bit `StoreId`, rendered as canonical lowercase hex. `nil` for a
    /// persisted v2 epoch scope.
    public let storeID: String?

    public init(serial: String, epoch: UInt32) {
        self.serial = serial
        self.epoch = epoch
        self.storeID = nil
    }

    public init(serial: String, storeID: String) {
        self.serial = serial
        self.epoch = 0
        self.storeID = storeID
    }
}

/// The durable link between a library route and its copy on a device:
/// `{serial, epoch, id}` — the v2 replacement for the bare `deviceObjectID`
/// a v1 library stored (which silently matched *any* connected device).
///
/// The link is meaningful **only against the device it was minted on, in the
/// era it was minted in**: ``matches(_:)`` is the validity predicate — V6's
/// badge/adoption logic consumes it, and the upload path uses it so a
/// replace-by-id can never target another device's (or another era's) object.
/// A v1 flat link (object id without serial/epoch) fails the predicate by
/// construction — it decodes as no link at all, and V6's CRC adoption is what
/// re-links those records properly.
public struct DeviceRouteLink: Hashable, Sendable {
    /// DIS serial of the device the upload committed on.
    public let serial: String
    /// That device's store epoch at commit time.
    public let epoch: UInt32
    public let storeID: String?
    /// The device object id the route is stored under (spec §4.1).
    public let objectID: DeviceObjectID

    public init(serial: String, epoch: UInt32, objectID: DeviceObjectID) {
        self.serial = serial
        self.epoch = epoch
        self.storeID = nil
        self.objectID = objectID
    }

    public init(serial: String, storeID: String, objectID: DeviceObjectID) {
        self.serial = serial
        self.epoch = 0
        self.storeID = storeID
        self.objectID = objectID
    }

    public init(scope: LibraryScope, objectID: DeviceObjectID) {
        self.serial = scope.serial
        self.epoch = scope.epoch
        self.storeID = scope.storeID
        self.objectID = objectID
    }

    /// The scope half of the link.
    public var scope: LibraryScope {
        if let storeID { return LibraryScope(serial: serial, storeID: storeID) }
        return LibraryScope(serial: serial, epoch: epoch)
    }

    /// The validity predicate (#769): the link speaks for the connected device
    /// only when **all** of serial and epoch match its current identity — a
    /// link minted on another device, or in a previous era of this one, is
    /// silent (no badge, no replace-by-id; V6 consumes this for both).
    public func matches(_ scope: LibraryScope) -> Bool {
        serial == scope.serial && self.scope == scope
    }
}

extension DeviceInfo {
    /// The library scope this identity read establishes, or `nil` when it
    /// can't: a missing epoch (v1 peer, short/torn read — `storeEpoch` is
    /// deliberately never defaulted) or an empty DIS serial. `nil` is the
    /// fail-closed input: no scope, no `ackRides`, no reconcile writes (#769).
    public var libraryScope: LibraryScope? {
        guard !serial.isEmpty else { return nil }
        if let storeID { return LibraryScope(serial: serial, storeID: storeID) }
        guard let storeEpoch else { return nil }
        return LibraryScope(serial: serial, epoch: storeEpoch)
    }
}
