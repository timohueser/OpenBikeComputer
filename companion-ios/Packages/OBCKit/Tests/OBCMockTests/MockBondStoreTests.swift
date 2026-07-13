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

        store.save(BondRecord(deviceName: "Summit"))
        XCTAssertTrue(control.bonded)
        // The saved name is served back: the bond record is the *desired*
        // name, which deliberately diverges from `deviceInfo` when a rename's
        // config write failed — the gap the reconcile pass detects (#361).
        XCTAssertEqual(store.load(), BondRecord(deviceName: "Summit"))

        store.clear()
        XCTAssertFalse(control.bonded)
        XCTAssertNil(store.load())
    }

    /// Scenario boots never `save` — the store serves the control's live
    /// identity, and `apply()` drops any saved name with the rest of the knobs.
    func testScenarioBootsServeTheLiveIdentityUntilASave() {
        let control = MockControl(scenario: .happyPath)
        let store = MockBondStore(control: control)
        XCTAssertEqual(store.load(), BondRecord(deviceName: control.deviceInfo.name))

        control.bondedName = "Stale"
        control.apply(.happyPath)
        XCTAssertEqual(store.load(), BondRecord(deviceName: control.deviceInfo.name))
    }
}
