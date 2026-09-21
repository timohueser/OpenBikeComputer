#if DEBUG
import Foundation
import OBCTransport

/// The mock `BondStore`: a view onto `MockControl.bonded`, so the scenario preset decides the
/// launch branch and the dev panel can flip it live to replay first-run pairing. Nothing
/// persists; every launch starts from the scenario, which is what automation wants.
public struct MockBondStore: BondStore {
    private let control: MockControl

    public init(control: MockControl) {
        self.control = control
    }

    public func load() -> BondRecord? {
        control.bonded ? BondRecord(deviceName: control.bondedName ?? control.deviceInfo.name) : nil
    }

    /// A save keeps the record's name. That name is the desired one: after a rename whose
    /// config write failed it diverges from `deviceInfo` on purpose, which is what the
    /// reconcile pass detects. A scenario boot has no saved name and falls back to `deviceInfo`.
    public func save(_ record: BondRecord) {
        control.bonded = true
        control.bondedName = record.deviceName
    }

    public func clear() {
        control.bonded = false
        control.bondedName = nil
    }
}
#endif
