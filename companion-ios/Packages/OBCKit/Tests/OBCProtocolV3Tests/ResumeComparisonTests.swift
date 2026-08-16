import Foundation
import Testing

@testable import OBCProtocolV3

/// `Device_Object_Vectors_v2.md` §2.1: "A resume vector proves the comparison end to end: the
/// client's retained prefix CRC against the acceptance's field, on both the matching and
/// mismatching branches."
///
/// This is the one place the suite's fixtures interlock rather than standing alone — the upload
/// stream frames, the acceptance, the checkpoint responses and the StartUpload descriptor all
/// describe the *same* 3,000-byte object — so it is also the strongest available check that this
/// codec agrees with the fixture author about what the bytes mean, not merely about where the
/// fields sit.
@Suite("Device Object v3 — resume prefix CRC")
struct ResumeComparisonTests {
    /// The three upload data frames, concatenated in offset order.
    static func uploadStream() throws -> [UInt8] {
        var assembled: [UInt8] = []
        for name in ["upload-first-frame", "upload-middle-frame", "upload-final-frame"] {
            let frame = try StreamFrame.decode(try streamRecord(name))
            guard case .data(let payload) = frame.body else {
                throw VectorError("\(name) is not a data frame")
            }
            guard frame.absoluteOffset == UInt64(assembled.count) else {
                throw VectorError("\(name) is not at the next offset")
            }
            assembled += payload
        }
        return assembled
    }

    static func streamRecord(_ name: String) throws -> [UInt8] {
        let url = DeviceObjectVectors.suiteDirectory
            .appendingPathComponent("streams/\(name).json")
        guard let data = FileManager.default.contents(atPath: url.path),
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { throw VectorError("missing stream fixture \(name)") }
        return try (json["record"] as? String ?? "").hexBytes
    }

    static func controlFrame(_ relativePath: String) throws -> ControlFrame {
        let url = DeviceObjectVectors.suiteDirectory.appendingPathComponent(relativePath)
        guard let data = FileManager.default.contents(atPath: url.path),
            let json = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { throw VectorError("missing control fixture \(relativePath)") }
        return try ControlFrame.decode(try (json["frame"] as? String ?? "").hexBytes)
    }

    @Test("the resumed acceptance's prefix CRC is the finalized CRC of exactly that durable prefix")
    func matchingBranch() throws {
        let stream = try Self.uploadStream()
        let accepted = try Self.controlFrame("controls/upload-accepted-resumed.json")
        guard case .uploadAccepted(let acceptance) = accepted.body else {
            throw VectorError("upload-accepted-resumed is not an acceptance")
        }
        #expect(acceptance.flags.contains(.resumedWork))
        let durable = Int(acceptance.durableNextOffset)
        #expect(durable > 0 && durable <= stream.count)

        // §6.2: "ordinary finalized CRC-32/IEEE over exactly bytes [0, durable_next_offset)".
        let retained = CRC32IEEE.checksum(stream[0..<durable])
        #expect(retained == acceptance.finalizedPrefixCRC32)

        // The checkpoint response reports the same quantity for the same prefix, and its sequence
        // starts at 1 for the first durable checkpoint of the work record.
        let checkpoint = try Self.controlFrame("controls/checkpoint-accepted-sequence-1.json")
        guard case .checkpointAccepted(let response) = checkpoint.body else {
            throw VectorError("checkpoint-accepted-sequence-1 is not a checkpoint response")
        }
        #expect(response.durableNextOffset == acceptance.durableNextOffset)
        #expect(response.finalizedPrefixCRC32 == retained)
        #expect(response.checkpointSequence == 1)
    }

    @Test("a retained prefix that differs by one byte fails the comparison")
    func mismatchingBranch() throws {
        var stream = try Self.uploadStream()
        let accepted = try Self.controlFrame("controls/upload-accepted-resumed.json")
        guard case .uploadAccepted(let acceptance) = accepted.body else {
            throw VectorError("upload-accepted-resumed is not an acceptance")
        }
        let durable = Int(acceptance.durableNextOffset)
        stream[durable - 1] ^= 0x01
        // §6.2: "mismatch requires restart at zero or AbortOperation, never concatenation onto an
        // unverified prefix."
        #expect(CRC32IEEE.checksum(stream[0..<durable]) != acceptance.finalizedPrefixCRC32)
    }

    @Test("the final checkpoint and the StartUpload descriptor describe the same whole object")
    func wholeObjectAgreement() throws {
        let stream = try Self.uploadStream()
        let start = try Self.controlFrame("controls/start-upload-create-route.json")
        guard case .startUpload(let request) = start.body else {
            throw VectorError("start-upload-create-route is not a StartUpload")
        }
        #expect(Int(request.declaredLength) == stream.count)
        #expect(request.expectedCRC32 == CRC32IEEE.checksum(stream))

        // §6.2: the last checkpoint sits at the declared end rather than on a granule boundary.
        let last = try Self.controlFrame("controls/checkpoint-accepted-sequence-3.json")
        guard case .checkpointAccepted(let response) = last.body else {
            throw VectorError("checkpoint-accepted-sequence-3 is not a checkpoint response")
        }
        #expect(response.durableNextOffset == request.declaredLength)
        #expect(response.finalizedPrefixCRC32 == request.expectedCRC32)
        #expect(response.checkpointSequence == 3)
    }

    @Test("the download acceptance describes the same object the download frames carry")
    func downloadAgreement() throws {
        var assembled: [UInt8] = []
        for name in ["download-first-frame", "download-middle-frame", "download-final-frame"] {
            let frame = try StreamFrame.decode(try Self.streamRecord(name))
            guard case .data(let payload) = frame.body else {
                throw VectorError("\(name) is not a data frame")
            }
            #expect(frame.direction == .download)
            #expect(frame.absoluteOffset == UInt64(assembled.count))
            assembled += payload
        }
        let accepted = try Self.controlFrame("controls/download-accepted.json")
        guard case .downloadAccepted(let acceptance) = accepted.body else {
            throw VectorError("download-accepted is not an acceptance")
        }
        #expect(Int(acceptance.totalLength) == assembled.count)
        #expect(acceptance.wholeSourceCRC32 == CRC32IEEE.checksum(assembled))
        // §7: "The accepted start offset always equals the offset the request asked for."
        #expect(acceptance.acceptedStartOffset == 0)

        let finish = try Self.controlFrame("controls/finish-download-request.json")
        guard case .finishDownload(let request) = finish.body else {
            throw VectorError("finish-download-request is not a FinishDownload")
        }
        #expect(request.receivedWholeSourceLength == acceptance.totalLength)
        #expect(request.wholeSourceCRC32 == acceptance.wholeSourceCRC32)
    }
}
