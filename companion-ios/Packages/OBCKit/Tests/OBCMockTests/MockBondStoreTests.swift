import XCTest
import OBCTransport
@testable import OBCMock

/// The mock side of the B2 bond seam: scenario presets carry the bond bit and
/// `MockBondStore` is a live view onto `MockControl`.
final class MockBondStoreTests: XCTestCase {
    func testPairingScenariosBootUnbonded() {
        let unbonded: [Scenario] = [
            .noDevice, .pairingTimeout, .pairingRejected, .bluetoothOff, .permissionDenied,
        ]
        for scenario in Scenario.allCases {
            XCTAssertEqual(
                scenario.preset.bonded, !unbonded.contains(scenario),
                "\(scenario) bond bit"
            )
        }
    }

    func testControlPicksUpTheBondBitFromPresets() {
        let control = MockControl(scenario: .noDevice)
        XCTAssertFalse(control.bonded)
        control.apply(.happyPath)
        XCTAssertTrue(control.bonded)
        control.apply(.pairingTimeout)
        XCTAssertFalse(control.bonded)
    }

    func testStoreIsALiveViewOntoTheControl() {
        let control = MockControl(scenario: .noDevice)
        let store = MockBondStore(control: control)

        XCTAssertNil(store.load())

        store.save(BondRecord(deviceName: "whatever"))
        XCTAssertTrue(control.bonded)
        // The served name is the control's live identity, not the saved string.
        XCTAssertEqual(store.load(), BondRecord(deviceName: control.deviceInfo.name))

        store.clear()
        XCTAssertFalse(control.bonded)
        XCTAssertNil(store.load())
    }
}
