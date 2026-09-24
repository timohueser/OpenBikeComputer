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

    @Test("Metadata remains readable in an all-kind listing and cannot be uploaded")
    func metadataOwnership() throws {
        let bytes = try Vectors.frame(named: "list-response-metadata")
        guard case .list(let page) = try ControlResponse(decoding: bytes) else {
            Issue.record("Expected a catalog page")
            return
        }
        #expect(page.entries.map(\.kind) == [.metadata])
        let request = PutRequest(payloadLength: 112, payloadCRC32: 0, kind: .metadata, displayName: "")
        #expect(throws: WireError.self) {
            try ControlRequest.put(request).frame(requestID: RequestID(rawValue: 1)!)
        }
    }

    @Test("A PUT shortens a long multi-byte display name to 48 bytes on a character boundary")
    func longDisplayNameIsShortened() throws {
        let full = "Grimselpass → Furkapass → Oberalppass über Andermatt und Disentis"
        let request = PutRequest(payloadLength: 1, payloadCRC32: 0, kind: .route, displayName: full)
        let name = request.displayName
        #expect(name.utf8.count <= FlatStoreV4.maximumDisplayNameLength)
        #expect(Array(full).starts(with: Array(name)))
        let next = full[full.index(full.startIndex, offsetBy: name.count)]
        #expect(name.utf8.count + String(next).utf8.count > FlatStoreV4.maximumDisplayNameLength)
        _ = try ControlRequest.put(request).frame(requestID: RequestID(rawValue: 1)!).encode()
    }

    @Test("Accepted Assistant routes remain readable and undefined catalog flags fail")
    func assistantCatalogFlags() throws {
        var bytes = try Vectors.frame(named: "list-response-two-entries")
        guard case .list(let original) = try ControlResponse(decoding: bytes) else {
            Issue.record("Expected a catalog page")
            return
        }
        let routeFlagsOffset = FlatStoreV4.controlHeaderLength + 24 + 30
        bytes[routeFlagsOffset] = 1 << 3
        guard case .list(let accepted) = try ControlResponse(decoding: bytes) else {
            Issue.record("Expected the accepted route catalog page")
            return
        }
        #expect(accepted.entries.count == 2)
        #expect(accepted.entries[0].kind == .route)
        #expect(accepted.entries[0].objectID == original.entries[0].objectID)
        #expect(accepted.entries[0].revision == original.entries[0].revision)
        #expect(accepted.entries[0].flags == .assistantAccepted)
        #expect(accepted.entries[1] == original.entries[1])
        for bit in 4..<16 {
            let flags = UInt16(1 << 3) | UInt16(1 << bit)
            bytes[routeFlagsOffset] = UInt8(truncatingIfNeeded: flags)
            bytes[routeFlagsOffset + 1] = UInt8(truncatingIfNeeded: flags >> 8)
            #expect(throws: WireError.invalidFlags) { try ControlResponse(decoding: bytes) }
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
            } else if target == "controlResponse" {
                #expect(throws: WireError.self, "\(entry)") { try ControlResponse(decoding: bytes) }
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

        let archive = ControlRequest.archiveRide(
            storeID: try StoreID(bytes: Vectors.hex("8f2c41d96b074ea3b1559c207de83466")),
            objectID: ObjectID(rawValue: 2), revision: Revision(rawValue: 1),
            payloadLength: 42_137, payloadCRC32: 0x9C4A_7E21)
        #expect(try archive.frame(requestID: RequestID(rawValue: 0x2A09)!).encode()
            == Vectors.frame(named: "archive-ride-request"))
        let stamped = try Vectors.frame(named: "archive-ride-response-stamped")
        guard case .archiveRide(let result) = try ControlResponse(
            decoding: stamped, expectedOpcode: .archiveRide, expectedRequestID: RequestID(rawValue: 0x2A09)!
        ) else { Issue.record("Expected an archive receipt result"); return }
        #expect(result.timestamp == 0x65000000)
        #expect(throws: WireError.self) {
            try ControlResponse(decoding: stamped, expectedRequestID: RequestID(rawValue: 0x2A08)!)
        }

        let stream = try StreamRecord(
            requestID: requestID, offset: 40_960,
            payload: Data((0..<1_024).map { UInt8($0 % 251) }))
        let expectedStream = try Vectors.record(named: "stream-frame-of-section-3-11")
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
