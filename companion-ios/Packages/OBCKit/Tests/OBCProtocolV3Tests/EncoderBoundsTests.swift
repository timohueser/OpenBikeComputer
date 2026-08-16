import Foundation
import Testing

import OBCProtocolV3

/// §1's limits bind the **encoder** as well as the decoder. A frame whose complete encoding exceeds
/// its bound is *unsendable*, and §1 is explicit that a client "MUST NOT truncate, split, or drop a
/// field to make it fit" — so the only correct behaviour is to refuse, which needs an error channel.
///
/// This suite deliberately imports the module **without** `@testable`, so everything it touches is
/// the public surface an application sees. Nothing here may trap: a codec that ships inside the
/// companion app must turn every out-of-range value into a returned error, never a process-fatal
/// narrowing conversion or a precondition failure.
@Suite("Device Object v3 — encoder bounds")
struct EncoderBoundsTests {
    static func requestId() throws -> RequestId { try #require(RequestId(1)) }
    static func sessionId() throws -> SessionId { try #require(SessionId(17)) }

    /// §16: Echo's maximum is the negotiated control frame less the 16-byte header, "which is
    /// exactly the bound every control frame already has".
    @Test("an over-long Echo payload is refused rather than emitted")
    func echoBeyondTheControlBound() throws {
        let frame = ControlFrame(
            opcode: .echo, flags: [], requestId: try Self.requestId(),
            body: .echoRequest([UInt8](repeating: 0xAB, count: 600)))
        #expect(throws: WireFault.self) { _ = try frame.encoded() }

        // The largest sendable Echo still encodes, and to exactly 512 bytes.
        let atTheBound = ControlFrame(
            opcode: .echo, flags: [], requestId: try Self.requestId(),
            body: .echoRequest([UInt8](repeating: 0xAB, count: WireLimits.maximumControlPayload)))
        #expect(try atTheBound.encoded().count == WireLimits.maximumControlFrame)
    }

    /// §12: "text length, at most 64".
    @Test("an over-long diagnostic text is refused rather than emitted")
    func errorTextBeyond64() throws {
        func body(textBytes: Int) -> ErrorBody {
            ErrorBody(
                rawCategory: ErrorCategory.internal.rawValue, detail: 1, rawGuidance: 0,
                text: [UInt8](repeating: 0x78, count: textBytes))
        }
        #expect(throws: WireFault.self) { _ = try body(textBytes: 100).encoded() }
        #expect(try body(textBytes: WireLimits.errorTextCeiling).encoded().count == 48 + 64)

        // …and through the frame that carries it.
        let frame = ControlFrame(
            opcode: .finishUpload, flags: [.response, .error], requestId: try Self.requestId(),
            body: .error(body(textBytes: 100)))
        #expect(throws: WireFault.self) { _ = try frame.encoded() }
    }

    /// §1: the maximum stream frame, header included, is 4,096 bytes.
    @Test("an over-long stream payload is refused rather than emitted")
    func streamBeyond4096() throws {
        func frame(payload: Int) throws -> StreamFrame {
            StreamFrame(
                sessionId: try Self.sessionId(), absoluteOffset: 0, direction: .upload, flags: [],
                body: .data([UInt8](repeating: 0x5A, count: payload)))
        }
        #expect(throws: WireFault.self) { _ = try frame(payload: 5000).encoded() }
        let atTheBound = try frame(
            payload: WireLimits.maximumStreamFrame - WireLimits.streamHeaderBytes)
        #expect(try atTheBound.encoded().count == WireLimits.maximumStreamFrame)
    }

    /// §13: a receiver rejects `offset + length` overflow, so an encoder must not produce it.
    @Test("a stream frame whose offset plus length overflows is refused")
    func streamOffsetOverflow() throws {
        let frame = StreamFrame(
            sessionId: try Self.sessionId(), absoluteOffset: UInt64.max, direction: .upload,
            flags: [], body: .data([0x01, 0x02]))
        #expect(throws: WireFault.self) { _ = try frame.encoded() }
    }

    /// §16: the config block's device-name length is `0` through `32`.
    @Test("an over-long device name is refused rather than emitted")
    func configNameBeyond32() throws {
        func block(nameBytes: Int) -> DeviceConfigBlock {
            DeviceConfigBlock(
                codecVersion: 1, unitFlags: [], weatherRefresh: .off,
                nameBytes: [UInt8](repeating: 0x61, count: nameBytes))
        }
        #expect(throws: WireFault.self) { _ = try block(nameBytes: 33).encoded() }
        #expect(try block(nameBytes: 32).encoded().count == DeviceConfigBlock.payloadBytes)
        #expect(try block(nameBytes: 0).encoded().count == DeviceConfigBlock.payloadBytes)
    }

    /// Every refusal above is a `WireFault` a caller can act on, in the categories §12 already
    /// defines for framing problems — not a bespoke error type and not a trap.
    @Test("refusals are ordinary WireFaults")
    func refusalsAreTypedFaults() throws {
        let frame = ControlFrame(
            opcode: .echo, flags: [], requestId: try Self.requestId(),
            body: .echoRequest([UInt8](repeating: 0, count: 600)))
        do {
            _ = try frame.encoded()
            Issue.record("the over-long frame encoded")
        } catch let fault as WireFault {
            #expect(fault.category == .invalidFrame)
            #expect(fault.detailName == "frameBounds")
        }
    }
}

/// M3's other half: the public surface offers no way to build a malformed value that would later
/// trap. Opaque identities and the catalog cursor are 16 bytes by construction, because the only
/// public initializer is failable.
@Suite("Device Object v3 — public API is trap-free")
struct PublicSurfaceTests {
    @Test("opaque identities reject any length but 16")
    func identityLengths() {
        for count in [0, 1, 15, 17, 32] {
            let bytes = [UInt8](repeating: 0xAB, count: count)
            #expect(StoreId(bytes: bytes) == nil)
            #expect(OperationId(bytes: bytes) == nil)
            #expect(DraftPartRef(bytes: bytes) == nil)
            #expect(DeviceSerial(bytes: bytes) == nil)
        }
        #expect(StoreId(bytes: [UInt8](repeating: 0xAB, count: 16)) != nil)
    }

    /// The cursor's accessors index fixed offsets, so a short cursor would trap on read. It cannot
    /// be built from public API.
    @Test("a catalog cursor rejects any length but 16, and its accessors are then total")
    func cursorLengths() throws {
        for count in [0, 8, 12, 15, 17] {
            #expect(CatalogCursor(bytes: [UInt8](repeating: 0, count: count)) == nil)
        }
        let cursor = try #require(CatalogCursor(bytes: [UInt8](repeating: 0xFF, count: 16)))
        _ = cursor.revision
        _ = cursor.nextEntryIndex
        _ = cursor.objectKindCode
        _ = cursor.checksum
        let storeId = try #require(StoreId(bytes: [UInt8](repeating: 0x11, count: 16)))
        _ = cursor.expectedChecksum(storeId: storeId)
    }

    /// Nonzero-by-construction capabilities: §2 and §3 both make zero illegal.
    @Test("session and request identifiers reject zero")
    func nonzeroIdentifiers() {
        #expect(SessionId(0) == nil)
        #expect(RequestId(0) == nil)
        #expect(SessionId(1) != nil)
        #expect(RequestId(1) != nil)
    }

    /// Decoding stays total across the whole public entry-point set: no input, however malformed,
    /// may do anything but return a value or throw. This walks a spread of adversarial byte
    /// patterns through every public decoder.
    @Test("every public decoder is total over adversarial input")
    func decodersAreTotal() {
        var patterns: [[UInt8]] = [[], [0x00], [0xFF]]
        for length in [1, 4, 15, 16, 17, 47, 48, 49, 63, 64, 96, 112, 128, 192, 512, 4096] {
            patterns.append([UInt8](repeating: 0x00, count: length))
            patterns.append([UInt8](repeating: 0xFF, count: length))
            patterns.append((0..<length).map { UInt8($0 % 251) })
        }
        // A well-formed control header with a hostile payload underneath it.
        for payloadLength in [0, 1, 48, 176, 496] {
            var writer: [UInt8] = Array("OBCP".utf8) + [3, 0]
            writer += [0x00, 0x01]  // opcode 0x0100
            writer += [0x00, 0x00]  // flags
            writer += [UInt8(payloadLength & 0xFF), UInt8(payloadLength >> 8)]
            writer += [1, 0, 0, 0]  // RequestId
            writer += [UInt8](repeating: 0xFF, count: payloadLength)
            patterns.append(writer)
        }

        for bytes in patterns {
            _ = try? ControlFrame.decode(bytes)
            _ = try? StreamFrame.decode(bytes)
            _ = try? ErrorBody.decode(bytes)
            _ = try? CapabilitiesPage.decode(bytes)
            _ = try? SubjectEntry.decode(bytes)
            _ = try? DeviceConfigBlock.decode(bytes)
            _ = try? DeviceStatus.decode(bytes)
            _ = try? Hello.decode(bytes)
            _ = try? QueryOperationState.decode(bytes)
            _ = try? WeatherRequestContext.decode(bytes)
            _ = try? SetClockRequest.decode(bytes)
            _ = try? ClockStatus.decode(bytes)
            _ = try? ForgetBondRequest.decode(bytes)
            _ = try? StartUploadRequest.decode(bytes)
            _ = try? StartDownloadRequest.decode(bytes)
            _ = try? QueryCatalogRequest.decode(bytes)
            _ = try? QueryDraftRequest.decode(bytes)
            _ = try? CatalogPage.decode(bytes, more: false)
            _ = try? CatalogPage.decode(bytes, more: true)
            _ = try? DraftPage.decode(bytes, more: false)
            _ = try? MetadataEnvelope.decode(bytes, maximumEncodedLength: 96)
            _ = try? MetadataEnvelope.decode(bytes, maximumEncodedLength: 128)
        }
    }
}
