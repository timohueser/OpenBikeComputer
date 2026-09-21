import XCTest
import OBCDomain
@testable import OBCTransport

/// Proves the domain and transport layers build and test under `swift test`, with no simulator
/// and no app target.
final class DeviceInfoTests: XCTestCase {
    func testDeviceInfoIsEquatableByValue() {
        let a = DeviceInfo(name: "OBC", firmwareVersion: "1.0.0")
        let b = DeviceInfo(name: "OBC", firmwareVersion: "1.0.0")
        let c = DeviceInfo(name: "OBC", firmwareVersion: "1.0.1")
        XCTAssertEqual(a, b)
        XCTAssertNotEqual(a, c)
    }
}
