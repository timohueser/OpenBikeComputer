import Foundation
import Testing
@testable import OBCProtocolV4

@Suite("FLAT store protocol v4 vectors")
struct FlatStoreVectorTests {
    @Test("Every request control vector passes the outgoing check")
    func requests() throws {
        for entry in try Vectors.entries(in: "controls") {
            let object = try Vectors.object(entry)
            guard object["direction"] as? String == "request" else { continue }
            let frame = try ControlFrame(decodingRequest: Vectors.hex(object["frame"]))
            let opcode = (object["opcode"] as? [String: Any])?["value"] as? Int
            #expect(Int(frame.opcode.rawValue) == opcode, "\(entry)")
        }
    }

    @Test("A PUT of device-owned metadata is refused")
    func metadataCannotBeUploaded() throws {
        var bytes = try Vectors.frame(named: "put-create-request")
        _ = try ControlFrame(decodingRequest: bytes)
        // The kind follows the object, revision, length and CRC fields of the PUT body.
        bytes[FlatStoreV4.controlHeaderLength + 28] = UInt8(ObjectKind.metadata.rawValue)
        #expect(throws: WireError.invalidCombination) { try ControlFrame(decodingRequest: bytes) }
    }

    @Test("Every stream vector decodes to its header fields")
    func streams() throws {
        for entry in try Vectors.entries(in: "streams") {
            let object = try Vectors.object(entry)
            let record = try StreamRecord(decoding: Vectors.hex(object["record"]))
            #expect(Int(record.requestID.rawValue) == object["requestId"] as? Int, "\(entry)")
            #expect(String(record.offset) == object["offset"] as? String, "\(entry)")
            #expect(record.payload.count == object["payloadLength"] as? Int, "\(entry)")
        }
    }

    @Test("Every pinned malformed request or stream record is refused")
    func negatives() throws {
        for entry in try Vectors.entries(in: "negative") {
            let object = try Vectors.object(entry)
            let bytes = try Vectors.hex(object["bytes"])
            switch object["target"] as? String {
            case "streamRecord":
                #expect(throws: WireError.self, "\(entry)") { try StreamRecord(decoding: bytes) }
            case "controlRecord":
                #expect(throws: WireError.self, "\(entry)") { try ControlFrame(decodingRequest: bytes) }
            default:
                // Response checks belong to the Rust client.
                break
            }
        }
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
