import Foundation
import Testing
@testable import OBCProtocolV4

@Suite("FLAT store protocol v4 vectors")
struct FlatStoreVectorTests {
    @Test("Every positive control vector decodes and re-encodes byte-for-byte")
    func controls() throws {
        for entry in try Vectors.entries(in: "controls") {
            let object = try Vectors.object(entry)
            let bytes = try Vectors.hex(object["frame"])
            let direction: ControlDirection = object["direction"] as? String == "request" ? .request : .response
            let frame = try ControlFrame(decoding: bytes, direction: direction)
            #expect(frame.encode() == bytes, "\(entry)")
            if case .response = direction {
                if frame.isError {
                    #expect(throws: WireError.self) { try ControlResponse(decoding: bytes) }
                } else {
                    _ = try ControlResponse(decoding: bytes)
                }
            }
        }
    }

    @Test("Every stream vector decodes and re-encodes byte-for-byte")
    func streams() throws {
        for entry in try Vectors.entries(in: "streams") {
            let object = try Vectors.object(entry)
            let bytes = try Vectors.hex(object["record"])
            #expect(try StreamRecord(decoding: bytes).encode() == bytes, "\(entry)")
        }
    }

    @Test("Every pinned error decodes to its typed remote error")
    func errors() throws {
        for entry in try Vectors.entries(in: "errors") {
            let bytes = try Vectors.hex(try Vectors.object(entry)["frame"])
            let frame = try ControlFrame(decoding: bytes, direction: .response)
            #expect(frame.encode() == bytes, "\(entry)")
            #expect(throws: WireError.self, "\(entry)") {
                try ControlResponse(decoding: bytes)
            }
        }
    }

    @Test("Every pinned malformed record is refused")
    func negatives() throws {
        for entry in try Vectors.entries(in: "negative") {
            let object = try Vectors.object(entry)
            let bytes = try Vectors.hex(object["bytes"])
            let target = object["target"] as? String
            if target == "streamRecord" {
                #expect(throws: WireError.self, "\(entry)") { try StreamRecord(decoding: bytes) }
            } else {
                #expect(throws: WireError.self, "\(entry)") {
                    try ControlFrame(decoding: bytes, direction: .request)
                }
            }
        }
    }

    @Test("Request builders match the pinned examples")
    func requestBuilders() throws {
        let requestID = RequestID(rawValue: 0x2A01)!
        let put = PutRequest(
            payloadLength: 42_137, payloadCRC32: 0x9C4A_7E21, kind: .route,
            displayName: "Grimsel Loop")
        #expect(
            try ControlRequest.put(put).frame(requestID: requestID).encode()
                == Vectors.frame(named: "put-create-request"))

        let stream = try StreamRecord(
            requestID: requestID, offset: 40_960,
            payload: Data((0..<1_024).map { UInt8(($0 + 1) % 251) }))
        let expectedStream = try Vectors.record(named: "stream-frame-of-section-3-10")
        #expect(stream.encode() == expectedStream)
    }
}

private enum Vectors {
    static let root: URL = {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { url.deleteLastPathComponent() }
        return url.appendingPathComponent("specs/vectors/flat-store-v4")
    }()

    static func entries(in section: String) throws -> [String] {
        let manifest = try object(at: root.appendingPathComponent("manifest.json"))
        guard let rows = manifest[section] as? [[String: Any]] else { throw VectorFault.malformed }
        return try rows.map {
            guard let file = $0["file"] as? String else { throw VectorFault.malformed }
            return file
        }
    }

    static func object(_ relativePath: String) throws -> [String: Any] {
        try object(at: root.appendingPathComponent(relativePath))
    }

    static func frame(named name: String) throws -> Data {
        guard let entry = try entries(in: "controls").first(where: { $0.hasSuffix("/\(name).json") })
        else { throw VectorFault.missing }
        return try hex(try object(entry)["frame"])
    }

    static func record(named name: String) throws -> Data {
        guard let entry = try entries(in: "streams").first(where: { $0.hasSuffix("/\(name).json") })
        else { throw VectorFault.missing }
        return try hex(try object(entry)["record"])
    }

    static func hex(_ value: Any?) throws -> Data {
        guard let text = value as? String, text.count.isMultiple(of: 2) else { throw VectorFault.malformed }
        var bytes: [UInt8] = []
        var index = text.startIndex
        while index < text.endIndex {
            let end = text.index(index, offsetBy: 2)
            guard let byte = UInt8(text[index..<end], radix: 16) else { throw VectorFault.malformed }
            bytes.append(byte)
            index = end
        }
        return Data(bytes)
    }

    private static func object(at url: URL) throws -> [String: Any] {
        let data = try Data(contentsOf: url)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { throw VectorFault.malformed }
        return object
    }

    enum VectorFault: Error { case missing, malformed }
}
