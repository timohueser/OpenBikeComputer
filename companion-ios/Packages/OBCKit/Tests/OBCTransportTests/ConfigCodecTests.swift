import XCTest
import OBCDomain
@testable import OBCTransport

/// The provisional `Config` blob codec (Codecs/ConfigCodec.swift) — round-trip +
/// malformed-input behavior. When the wire layout is repinned, these tests are
/// the single spot that must move with it.
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

    /// A name past the u16 range must not wrap the length field into a
    /// corrupt/undersized blob — encode caps it at the 48-byte limit, so the
    /// blob stays well-formed and the trailing `units` survive.
    func testEncodeCapsOverLongNameToKeepBlobWellFormed() throws {
        let huge = String(repeating: "A", count: 70_000)   // ≥ 65536 bytes
        let blob = ConfigObjectCodec.encode(DeviceConfig(name: huge, units: .imperial))
        let decoded = try ConfigObjectCodec.decode(blob)
        XCTAssertEqual(decoded.name, String(repeating: "A", count: DeviceConfig.maxNameUTF8Bytes))
        XCTAssertEqual(decoded.name.utf8.count, DeviceConfig.maxNameUTF8Bytes)
        XCTAssertEqual(decoded.units, .imperial, "the length field didn't corrupt the blob")
    }

    /// The cap lands on a Character boundary, never mid-sequence: 17×3-byte
    /// scalars (51 B) truncate to the 16 that fit in 48 B — still valid UTF-8.
    func testEncodeCapNeverSplitsAMultiByteCharacter() throws {
        let name = String(repeating: "名", count: 17)   // 51 UTF-8 bytes
        let decoded = try ConfigObjectCodec.decode(
            ConfigObjectCodec.encode(DeviceConfig(name: name, units: .metric)))
        XCTAssertEqual(decoded.name, String(repeating: "名", count: 16))
        XCTAssertEqual(decoded.name.utf8.count, 48)
    }
}
