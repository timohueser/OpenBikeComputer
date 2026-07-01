import XCTest
import OBCDomain
@testable import OBCTransport

/// The provisional `Config` blob codec (Codecs/ConfigCodec.swift) — round-trip +
/// malformed-input behavior. Layout is S0-owned; when it's repinned these tests
/// are the single spot that must move with it.
final class ConfigCodecTests: XCTestCase {
    func testRoundTrip() throws {
        let config = DeviceConfig(name: "Trailhead", units: .imperial)
        XCTAssertEqual(try ProvisionalConfigCodec.decode(ProvisionalConfigCodec.encode(config)), config)
    }

    func testRoundTripUnicodeAndEmptyName() throws {
        for name in ["", "Rad 🚲 Tourenrechner", "名前"] {
            let config = DeviceConfig(name: name, units: .metric)
            XCTAssertEqual(try ProvisionalConfigCodec.decode(ProvisionalConfigCodec.encode(config)), config)
        }
    }

    func testDecodeRejectsTruncatedBlob() {
        XCTAssertThrowsError(try ProvisionalConfigCodec.decode(Data()))
        XCTAssertThrowsError(try ProvisionalConfigCodec.decode(Data([5])))          // no length hi-byte
        XCTAssertThrowsError(try ProvisionalConfigCodec.decode(Data([5, 0, 65])))   // name shorter than declared
    }

    func testDecodeToleratesSlicedData() throws {
        // Decode must respect `startIndex` (Data slices don't start at 0).
        let blob = ProvisionalConfigCodec.encode(DeviceConfig(name: "OBC", units: .metric))
        let sliced = (Data([0xFF, 0xFF]) + blob)[2...]
        XCTAssertEqual(try ProvisionalConfigCodec.decode(sliced).name, "OBC")
    }

    func testUnknownUnitsFallsBackToMetric() throws {
        var blob = ProvisionalConfigCodec.encode(DeviceConfig(name: "OBC", units: .metric))
        blob[blob.count - 1] = 0x7F   // an enum value future firmware might send
        XCTAssertEqual(try ProvisionalConfigCodec.decode(blob).units, .metric)
    }
}
