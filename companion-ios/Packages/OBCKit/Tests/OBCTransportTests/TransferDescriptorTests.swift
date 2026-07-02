import XCTest
@testable import OBCTransport

/// The control-plane descriptors that carry all transfer metadata (so the CoC is
/// raw bytes). Fixed-size, little-endian, trivially MCU-parseable — pinned by
/// firmware S0 (`obc-ble-interface-spec.md` §4.2/§4.3/§4.5). Byte-exactness against
/// the shared fixtures lives in `ProtocolVectorTests`; this suite covers the
/// round-trip + rejection behavior.
final class TransferDescriptorTests: XCTestCase {
    func testTransferControlRoundTrips() throws {
        for op in TransferControl.Op.allCases {
            let control = TransferControl(
                op: op, type: .route, objectID: 0xBEEF, totalLen: 123_456, crc32: 0xDEAD_C0DE, offset: 4_096
            )
            let encoded = control.encode()
            XCTAssertEqual(encoded.count, TransferControl.encodedLength)
            XCTAssertEqual(try TransferControl(decoding: encoded), control)
        }
    }

    func testTransferControlDefaultsToFreshTransfer() {
        let control = TransferControl(op: .upload, type: .ride, objectID: 1, totalLen: 10, crc32: 0)
        XCTAssertEqual(control.offset, 0)
        // A download request carries no length/CRC — the device's announce fills them.
        let request = TransferControl(op: .download, type: .rideList, objectID: 0)
        XCTAssertEqual(request.totalLen, 0)
        XCTAssertEqual(request.crc32, 0)
    }

    func testStatusMessageRoundTrips() throws {
        for status in TransferResult.Status.allCases {
            let msg = StatusMessage.transferResult(TransferResult(objectID: 7, status: status, committedOffset: 2_048))
            let encoded = msg.encode()
            XCTAssertEqual(encoded.count, 8)
            XCTAssertEqual(try StatusMessage(decoding: encoded), msg)
        }

        let store = StatusMessage.storeChanged(StoreChanged(type: .ride, revision: 42))
        XCTAssertEqual(store.encode().count, 6)
        XCTAssertEqual(try StatusMessage(decoding: store.encode()), store)

        for status in CommandResult.Status.allCases {
            let msg = StatusMessage.commandResult(CommandResult(command: 1, status: status))
            XCTAssertEqual(msg.encode().count, 4)
            XCTAssertEqual(try StatusMessage(decoding: msg.encode()), msg)
        }
    }

    func testUnknownStatusMessageIsIgnorableNotFatal() throws {
        // Forward compatibility (spec §4.3): an unknown discriminator decodes to
        // `.unknown`, never throws — the app skips it.
        XCTAssertEqual(try StatusMessage(decoding: Data([0x7F, 1, 2, 3])), .unknown(0x7F))
    }

    func testObjectStoreDigestRoundTrips() throws {
        let digest = ObjectStoreDigest(revision: 42, routeCount: 3, rideCount: 5)
        let encoded = digest.encode()
        XCTAssertEqual(encoded.count, ObjectStoreDigest.encodedLength)
        XCTAssertEqual(try ObjectStoreDigest(decoding: encoded), digest)
    }

    func testRejectsTruncated() {
        XCTAssertThrowsError(try TransferControl(decoding: Data([1, 2, 3]))) {
            XCTAssertEqual($0 as? DescriptorError, .truncated)
        }
        XCTAssertThrowsError(try StatusMessage(decoding: Data())) {
            XCTAssertEqual($0 as? DescriptorError, .truncated)
        }
        XCTAssertThrowsError(try StatusMessage(decoding: Data([1, 2]))) {
            XCTAssertEqual($0 as? DescriptorError, .truncated)
        }
        XCTAssertThrowsError(try ObjectStoreDigest(decoding: Data([1, 2]))) {
            XCTAssertEqual($0 as? DescriptorError, .truncated)
        }
    }

    func testRejectsUnknownEnums() {
        var control = TransferControl(op: .upload, type: .route, objectID: 1, totalLen: 1, crc32: 0).encode()
        control[control.startIndex] = 0x7F
        XCTAssertThrowsError(try TransferControl(decoding: control)) {
            XCTAssertEqual($0 as? DescriptorError, .unknownOp(0x7F))
        }

        var badType = TransferControl(op: .upload, type: .route, objectID: 1, totalLen: 1, crc32: 0).encode()
        badType[badType.startIndex + 1] = 0x7F
        XCTAssertThrowsError(try TransferControl(decoding: badType)) {
            XCTAssertEqual($0 as? DescriptorError, .unknownType(0x7F))
        }

        var result = StatusMessage.transferResult(TransferResult(objectID: 1, status: .committed, committedOffset: 0)).encode()
        result[result.startIndex + 3] = 0x7F
        XCTAssertThrowsError(try StatusMessage(decoding: result)) {
            XCTAssertEqual($0 as? DescriptorError, .unknownStatus(0x7F))
        }
    }
}
