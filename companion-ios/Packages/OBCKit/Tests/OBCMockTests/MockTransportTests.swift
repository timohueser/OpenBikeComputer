import XCTest
import OBCDomain
@testable import OBCMock

/// Tests run in Debug, so `#if DEBUG` is active and `OBCMock` is non-empty here.
final class MockTransportTests: XCTestCase {
    func testMockServesFixtureDeviceInfo() async throws {
        let transport = MockTransport()
        let info = try await transport.deviceInfo()
        XCTAssertEqual(info, DeviceInfo(name: "OBC (mock)", firmwareVersion: "0.0.0-mock"))
    }

    func testMockControlOverridesFixture() async throws {
        let custom = DeviceInfo(name: "OBC #42", firmwareVersion: "1.2.3")
        let transport = MockTransport(control: MockControl(deviceInfo: custom))
        let info = try await transport.deviceInfo()
        XCTAssertEqual(info, custom)
    }

    func testDebugBuildMarkerIsCompiledIn() {
        XCTAssertEqual(obcMockBuildMarker, "OBCMock:DEBUG-only")
    }
}
