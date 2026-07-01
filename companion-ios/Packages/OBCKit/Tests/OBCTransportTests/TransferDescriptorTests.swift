import XCTest
@testable import OBCTransport

/// The control-plane descriptors that carry all transfer metadata (so the CoC is
/// raw bytes). Fixed-size, little-endian, trivially MCU-parseable.
final class TransferDescriptorTests: XCTestCase {
    func testTransferStartRoundTrips() throws {
        let start = TransferStart(type: .route, objectID: 0xBEEF, totalLen: 123_456, crc32: 0xDEAD_C0DE, resumeOffset: 4_096)
        let encoded = start.encode()
        XCTAssertEqual(encoded.count, TransferStart.encodedLength)
        XCTAssertEqual(try TransferStart(decoding: encoded), start)
    }

    func testTransferStartDefaultsToFreshTransfer() {
        XCTAssertEqual(TransferStart(type: .ride, objectID: 1, totalLen: 10, crc32: 0).resumeOffset, 0)
    }

    func testTransferResultRoundTrips() throws {
        for status in [TransferResult.Status.committed, .crcMismatch, .aborted, .error] {
            let result = TransferResult(objectID: 7, status: status, committedOffset: 2_048)
            let encoded = result.encode()
            XCTAssertEqual(encoded.count, TransferResult.encodedLength)
            XCTAssertEqual(try TransferResult(decoding: encoded), result)
        }
    }

    func testRejectsTruncated() {
        XCTAssertThrowsError(try TransferStart(decoding: Data([1, 2, 3]))) {
            XCTAssertEqual($0 as? DescriptorError, .truncated)
        }
        XCTAssertThrowsError(try TransferResult(decoding: Data([1, 2]))) {
            XCTAssertEqual($0 as? DescriptorError, .truncated)
        }
    }

    func testRejectsUnknownEnums() {
        var start = TransferStart(type: .route, objectID: 1, totalLen: 1, crc32: 0).encode()
        start[start.startIndex] = 0x7F
        XCTAssertThrowsError(try TransferStart(decoding: start)) {
            XCTAssertEqual($0 as? DescriptorError, .unknownType(0x7F))
        }

        var result = TransferResult(objectID: 1, status: .committed, committedOffset: 0).encode()
        result[result.startIndex + 2] = 0x7F
        XCTAssertThrowsError(try TransferResult(decoding: result)) {
            XCTAssertEqual($0 as? DescriptorError, .unknownStatus(0x7F))
        }
    }
}
