#if DEBUG
import Foundation
import OBCTransport

/// The mock `BondStore`: a view onto `MockControl.bonded`, so the scenario
/// preset decides the launch branch (`noDevice` &co boot unpaired → D1) and the
/// dev panel can flip it live to replay first-run pairing. Nothing persists —
/// every launch starts from the scenario, which is exactly what automation wants.
public struct MockBondStore: BondStore {
    private let control: MockControl

    public init(control: MockControl) {
        self.control = control
    }

    public func load() -> BondRecord? {
        control.bonded ? BondRecord(deviceName: control.bondedName ?? control.deviceInfo.name) : nil
    }

    /// A save keeps the record's name (it's the *desired* name — after a rename
    /// whose config write failed it deliberately diverges from `deviceInfo`,
    /// which is what the reconcile pass detects, #361). Scenario boots have no
    /// saved name and fall back to `deviceInfo` live.
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
