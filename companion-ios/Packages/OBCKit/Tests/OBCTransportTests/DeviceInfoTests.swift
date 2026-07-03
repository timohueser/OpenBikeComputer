import XCTest
import OBCDomain
@testable import OBCTransport

/// Proves the domain + transport layers build and test with **no simulator and
/// no app target** (`swift test`).
final class DeviceInfoTests: XCTestCase {
    func testDeviceInfoIsEquatableByValue() {
        let a = DeviceInfo(name: "OBC", firmwareVersion: "1.0.0")
        let b = DeviceInfo(name: "OBC", firmwareVersion: "1.0.0")
        let c = DeviceInfo(name: "OBC", firmwareVersion: "1.0.1")
        XCTAssertEqual(a, b)
        XCTAssertNotEqual(a, c)
    }
}
