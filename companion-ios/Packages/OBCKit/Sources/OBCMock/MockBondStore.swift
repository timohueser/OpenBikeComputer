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
        control.bonded ? BondRecord(deviceName: control.deviceInfo.name) : nil
    }

    /// The name is served live from `control.deviceInfo` (so a rename stays in
    /// sync); saving just flips the bond bit.
    public func save(_ record: BondRecord) {
        control.bonded = true
    }

    public func clear() {
        control.bonded = false
    }
}
#endif
