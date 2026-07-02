import XCTest
import OBCDomain
@testable import OBCTransport

/// The provisional `Config` blob codec (Codecs/ConfigCodec.swift) — round-trip +
/// malformed-input behavior. Layout is S0-owned; when it's repinned these tests
/// are the single spot that must move with it.
final class ConfigCodecTests: XCTestCase {
    func testRoundTrip() throws {
        let config = DeviceConfig(name: "Trailhead", units: .imperial)
        XCTAssertEqual(try ConfigObjectCodec.decode(ConfigObjectCodec.encode(config)), config)
    }

    func testRoundTripUnicodeAndEmptyName() throws {
        for name in ["", "Rad 🚲 Tourenrechner", "名前"] {
            let config = DeviceConfig(name: name, units: .metric)
            XCTAssertEqual(try ConfigObjectCodec.decode(ConfigObjectCodec.encode(config)), config)
        }
    }

    func testDecodeRejectsTruncatedBlob() {
        XCTAssertThrowsError(try ConfigObjectCodec.decode(Data()))
        XCTAssertThrowsError(try ConfigObjectCodec.decode(Data([5])))          // no length hi-byte
        XCTAssertThrowsError(try ConfigObjectCodec.decode(Data([5, 0, 65])))   // name shorter than declared
    }

    func testDecodeToleratesSlicedData() throws {
        // Decode must respect `startIndex` (Data slices don't start at 0).
        let blob = ConfigObjectCodec.encode(DeviceConfig(name: "OBC", units: .metric))
        let sliced = (Data([0xFF, 0xFF]) + blob)[2...]
        XCTAssertEqual(try ConfigObjectCodec.decode(sliced).name, "OBC")
    }

    func testUnknownUnitsFallsBackToMetric() throws {
        var blob = ConfigObjectCodec.encode(DeviceConfig(name: "OBC", units: .metric))
        blob[blob.count - 1] = 0x7F   // an enum value future firmware might send
        XCTAssertEqual(try ConfigObjectCodec.decode(blob).units, .metric)
    }
}
