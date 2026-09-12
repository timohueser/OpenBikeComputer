import Foundation

/// A device serial and the full identity of its mounted store.
/// A different device or store leaves earlier library entries archival.
public struct LibraryScope: Hashable, Sendable {
    public let serial: String
    /// The exact 128-bit StoreId as canonical lowercase hexadecimal.
    public let storeID: String

    public init(serial: String, storeID: String) {
        self.serial = serial
        self.storeID = storeID
    }
}

/// A library route or trip's committed copy within one device store.
public struct DeviceRouteLink: Hashable, Sendable {
    public let serial: String
    public let storeID: String
    public let objectID: DeviceObjectID

    public init(serial: String, storeID: String, objectID: DeviceObjectID) {
        self.serial = serial
        self.storeID = storeID
        self.objectID = objectID
    }

    public init(scope: LibraryScope, objectID: DeviceObjectID) {
        self.init(serial: scope.serial, storeID: scope.storeID, objectID: objectID)
    }

    public var scope: LibraryScope {
        LibraryScope(serial: serial, storeID: storeID)
    }

    public func matches(_ scope: LibraryScope) -> Bool {
        self.scope == scope
    }
}

extension DeviceInfo {
    /// Missing or malformed identity disables scope-keyed device writes.
    public var libraryScope: LibraryScope? {
        guard !serial.isEmpty, let storeID, storeID.count == 32,
            storeID.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) })
        else { return nil }
        return LibraryScope(serial: serial, storeID: storeID)
    }
}
