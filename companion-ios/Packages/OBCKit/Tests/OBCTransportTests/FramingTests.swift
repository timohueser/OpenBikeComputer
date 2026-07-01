import XCTest
@testable import OBCTransport

/// The pure byte layer — CRC, frame codec, and reassembly — all exercised with no
/// hardware (issue B1 acceptance: "codec + framing unit tests pass without hardware").
final class FramingTests: XCTestCase {
    // MARK: CRC-32

    func testCRC32KnownVector() {
        // The canonical CRC-32/IEEE check value for the ASCII string "123456789".
        XCTAssertEqual(CRC32.checksum(Array("123456789".utf8)), 0xCBF4_3926)
        XCTAssertEqual(CRC32.checksum([]), 0)  // empty input
    }

    // MARK: Frame round-trip

    func testFrameRoundTrips() throws {
        let payload = Data((0..<200).map { UInt8($0 & 0xFF) })
        let frame = FrameCodec.encode(type: .ride, objectID: 0xBEEF, totalLen: 4096, offset: 128, payload: payload)

        let header = try FrameCodec.parseHeader(frame.prefix(FrameFormat.headerSize))
        let decodedPayload = frame.suffix(from: frame.startIndex + FrameFormat.headerSize)

        XCTAssertEqual(header.type, .ride)
        XCTAssertEqual(header.objectID, 0xBEEF)
        XCTAssertEqual(header.totalLen, 4096)
        XCTAssertEqual(header.offset, 128)
        XCTAssertEqual(header.chunkLen, UInt32(payload.count))
        XCTAssertEqual(Data(decodedPayload), payload)
        XCTAssertNoThrow(try FrameCodec.verify(header, payload: Data(decodedPayload)))
    }

    func testUnknownTypeByteRejected() {
        var frame = FrameCodec.encode(type: .route, objectID: 1, totalLen: 1, offset: 0, payload: Data([0x42]))
        frame[frame.startIndex] = 0x7F  // not a valid ObjectType
        XCTAssertThrowsError(try FrameCodec.parseHeader(frame)) {
            XCTAssertEqual($0 as? FramingError, .unknownType(0x7F))
        }
    }

    func testTruncatedHeaderRejected() {
        XCTAssertThrowsError(try FrameCodec.parseHeader(Data([1, 2, 3]))) {
            XCTAssertEqual($0 as? FramingError, .truncated)
        }
    }

    // MARK: CRC reject on corruption

    func testCorruptPayloadFailsCRC() throws {
        let payload = Data([1, 2, 3, 4, 5, 6, 7, 8])
        var frame = FrameCodec.encode(type: .route, objectID: 1, totalLen: 8, offset: 0, payload: payload)
        frame[FrameFormat.headerSize + 3] ^= 0xFF  // flip one payload byte

        let header = try FrameCodec.parseHeader(frame.prefix(FrameFormat.headerSize))
        let corruptPayload = Data(frame.suffix(from: frame.startIndex + FrameFormat.headerSize))
        XCTAssertThrowsError(try FrameCodec.verify(header, payload: corruptPayload)) {
            XCTAssertEqual($0 as? FramingError, .crcMismatch)
        }
    }

    func testAssemblerRejectsCorruptFrameAndDoesNotCommit() throws {
        let payload = Data([9, 8, 7, 6])
        var frame = FrameCodec.encode(type: .configBlob, objectID: 2, totalLen: 4, offset: 0, payload: payload)
        frame[frame.startIndex + FrameFormat.headerSize] ^= 0x01  // corrupt

        let header = try FrameCodec.parseHeader(frame.prefix(FrameFormat.headerSize))
        let corruptPayload = Data(frame.suffix(from: frame.startIndex + FrameFormat.headerSize))
        var assembler = TransferAssembler()
        XCTAssertThrowsError(try assembler.ingest(header: header, payload: corruptPayload)) {
            XCTAssertEqual($0 as? FramingError, .crcMismatch)
        }
        XCTAssertNil(assembler.object)          // rejected, never committed
        XCTAssertFalse(assembler.isComplete)
    }

    // MARK: Reassembly + offset-resume dedup

    func testAssemblerReconstructsMultiFrameObject() throws {
        let object = Data((0..<1000).map { UInt8(($0 * 7) & 0xFF) })
        var assembler = TransferAssembler()
        for frame in frames(of: object, type: .ride, objectID: 5, chunk: 64) {
            try assembler.ingest(header: frame.header, payload: frame.payload)
        }
        XCTAssertTrue(assembler.isComplete)
        XCTAssertEqual(assembler.object, object)
    }

    func testAssemblerToleratesResentBoundaryFrame() throws {
        let object = Data((0..<300).map { UInt8($0 & 0xFF) })
        let all = frames(of: object, type: .route, objectID: 1, chunk: 100)  // 3 frames
        var assembler = TransferAssembler()

        try assembler.ingest(header: all[0].header, payload: all[0].payload)  // offset 0
        try assembler.ingest(header: all[1].header, payload: all[1].payload)  // offset 100
        // Simulate a drop + resume that re-sends the already-committed frame 1.
        try assembler.ingest(header: all[1].header, payload: all[1].payload)  // duplicate → ignored
        try assembler.ingest(header: all[2].header, payload: all[2].payload)  // offset 200

        XCTAssertTrue(assembler.isComplete)
        XCTAssertEqual(assembler.object, object)
    }

    func testAssemblerRejectsGap() throws {
        let object = Data((0..<300).map { UInt8($0 & 0xFF) })
        let all = frames(of: object, type: .route, objectID: 1, chunk: 100)
        var assembler = TransferAssembler()
        try assembler.ingest(header: all[0].header, payload: all[0].payload)  // offset 0
        // Skip frame 1 → frame 2 is ahead of the committed offset.
        XCTAssertThrowsError(try assembler.ingest(header: all[2].header, payload: all[2].payload)) {
            XCTAssertEqual($0 as? FramingError, .truncated)
        }
    }

    // MARK: Helpers

    private func frames(of object: Data, type: ObjectType, objectID: UInt16, chunk: Int)
        -> [(header: FrameHeader, payload: Data)] {
        var out: [(FrameHeader, Data)] = []
        var offset = 0
        while offset < object.count {
            let end = Swift.min(offset + chunk, object.count)
            let payload = Data(object[(object.startIndex + offset)..<(object.startIndex + end)])
            let frame = FrameCodec.encode(
                type: type, objectID: objectID, totalLen: UInt32(object.count),
                offset: UInt32(offset), payload: payload
            )
            let header = try! FrameCodec.parseHeader(frame.prefix(FrameFormat.headerSize))
            out.append((header, payload))
            offset = end
        }
        return out
    }
}
