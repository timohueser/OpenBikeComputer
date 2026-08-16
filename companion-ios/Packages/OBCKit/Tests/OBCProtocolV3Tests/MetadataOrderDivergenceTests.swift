import Foundation
import Testing

@testable import OBCProtocolV3

/// The two ways a decoder can violate §2.2's frozen validation order by consulting the schema
/// registry too early. Both were real defects in the first draft of this codec, and both diverged
/// from the merged Rust codec on inputs no checked-in fixture covers — which is exactly why they
/// are pinned here as their own suite rather than folded into the fixture sweep.
///
/// The shared root cause was reading a *schema* fact off the wire during *canonical form*: once the
/// version byte decides either the rejection or the ceiling, a malformed envelope reports the fault
/// its lie selects instead of the fault it actually has.
@Suite("Device Object v3 — validation-order divergences")
struct MetadataOrderDivergenceTests {
    static func fault(_ body: () throws -> Void) -> WireFault? {
        do {
            try body()
            return nil
        } catch let fault as WireFault {
            return fault
        } catch {
            return nil
        }
    }

    /// An envelope whose version is unregistered **and** whose fields break canonical form. §2.2
    /// puts canonical form first, so the canonical-form fault is the one to report; an
    /// unregistered version is a schema-field rule and cannot pre-empt it.
    static func doublyInvalid(fields: [UInt8], fieldCount: UInt16) -> [UInt8] {
        var writer = ByteWriter()
        writer.u16(ObjectKind.route.rawValue)
        writer.u8(7)  // unregistered: not 1, 64 or 128
        writer.u8(0)
        writer.u16(UInt16(fields.count))
        writer.u16(fieldCount)
        writer.raw(fields)
        return writer.bytes
    }

    static func field(_ tag: UInt16, _ value: [UInt8]) -> [UInt8] {
        var writer = ByteWriter()
        writer.u16(tag)
        writer.u16(UInt16(value.count))
        writer.raw(value)
        return writer.bytes
    }

    /// The exact bytes the review used, which the merged Rust codec reports as `outOfOrderField`.
    @Test("an unregistered version does not mask an out-of-order base tag")
    func outOfOrderSurvivesABadVersion() throws {
        let bytes = try "010007000a00020002800100020180010001".hexBytes
        let fault = Self.fault {
            _ = try MetadataEnvelope.decode(
                bytes, maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        }
        #expect(fault?.category == .invalidDescriptor)
        #expect(fault?.detailName == "outOfOrderField")
    }

    @Test("an unregistered version does not mask a duplicate base tag")
    func duplicateSurvivesABadVersion() {
        let fields = Self.field(0x8001, [0x02]) + Self.field(0x8001, [0x02])
        let fault = Self.fault {
            _ = try MetadataEnvelope.decode(
                Self.doublyInvalid(fields: fields, fieldCount: 2),
                maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        }
        #expect(fault?.detailName == "duplicateField")
    }

    @Test("an unregistered version does not mask a zero base tag")
    func zeroBaseTagSurvivesABadVersion() {
        let fault = Self.fault {
            _ = try MetadataEnvelope.decode(
                Self.doublyInvalid(fields: Self.field(0x0000, [0x02]), fieldCount: 1),
                maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        }
        #expect(fault?.detailName == "noncanonicalMetadata")
    }

    @Test("an unregistered version does not mask a field-count disagreement")
    func fieldCountSurvivesABadVersion() {
        let fault = Self.fault {
            _ = try MetadataEnvelope.decode(
                Self.doublyInvalid(fields: Self.field(0x8001, [0x02]), fieldCount: 2),
                maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        }
        #expect(fault?.detailName == "noncanonicalMetadata")
    }

    /// The same masking through the whole-frame path, which is how a real peer would deliver it: a
    /// StartUpload descriptor carrying the out-of-order envelope above.
    @Test("the whole-frame path reports the canonical-form fault too")
    func wholeFramePathAgrees() throws {
        let envelope = try "010007000a00020002800100020180010001".hexBytes
        var payload = ByteWriter()
        payload.raw([UInt8](repeating: 0xA1, count: 16))  // OperationId
        payload.u16(ObjectKind.route.rawValue)
        payload.u8(TargetMode.create.rawValue)
        payload.u8(ResumePreference.restartAtZero.rawValue)
        payload.u64(0)  // LogicalObjectId
        payload.u64(0)  // expected Revision
        payload.u64(3000)  // declared length
        payload.u32(0x4636_A985)  // expected CRC
        payload.raw(envelope)

        var record = ByteWriter()
        record.raw(WireLimits.magic)
        record.u8(WireLimits.major)
        record.u8(WireLimits.minor)
        record.u16(Opcode.startUpload.rawValue)
        record.u16(0)
        record.u16(UInt16(payload.bytes.count))
        record.u32(1)
        record.raw(payload.bytes)

        let fault = Self.fault { _ = try ControlFrame.decode(record.bytes) }
        #expect(fault?.category == .invalidDescriptor)
        #expect(fault?.detailName == "outOfOrderField")
    }

    /// The second divergence: the class ceiling must come from the **call site**, not from the wire
    /// version. A 113-byte route Put envelope lying `version = 64` fits the 128-byte Put/patch
    /// ceiling its position imposes, so the fault it has is the version — not the size it would
    /// have exceeded if the decoder had believed the lie and applied the 96-byte catalog ceiling.
    @Test("the class ceiling comes from the call site, not from the wire version")
    func ceilingIsACallSiteFact() throws {
        let envelope = Self.putEnvelopeLyingAboutItsVersion()
        #expect(envelope.count == 113)
        #expect(envelope.count > SchemaClass.catalogProjection.envelopeCeiling)
        #expect(envelope.count <= SchemaClass.put.envelopeCeiling)

        let decoded = try MetadataEnvelope.decode(
            envelope, maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        let fault = Self.fault {
            try decoded.validated(kind: .route, schemaClass: .put, mutating: true)
        }
        #expect(fault?.category == .unsupportedCapability)
        #expect(fault?.detailName == "schemaVersion")
    }

    /// The same envelope through the whole-frame path.
    @Test("the whole-frame path reports the version, not the size")
    func ceilingIsACallSiteFactInAFrame() throws {
        var payload = ByteWriter()
        payload.raw([UInt8](repeating: 0xA1, count: 16))
        payload.u16(ObjectKind.route.rawValue)
        payload.u8(TargetMode.create.rawValue)
        payload.u8(ResumePreference.restartAtZero.rawValue)
        payload.u64(0)
        payload.u64(0)
        payload.u64(3000)
        payload.u32(0x4636_A985)
        payload.raw(Self.putEnvelopeLyingAboutItsVersion())

        var record = ByteWriter()
        record.raw(WireLimits.magic)
        record.u8(WireLimits.major)
        record.u8(WireLimits.minor)
        record.u16(Opcode.startUpload.rawValue)
        record.u16(0)
        record.u16(UInt16(payload.bytes.count))
        record.u32(1)
        record.raw(payload.bytes)

        let fault = Self.fault { _ = try ControlFrame.decode(record.bytes) }
        #expect(fault?.category == .unsupportedCapability)
        #expect(fault?.detailName == "schemaVersion")
    }

    /// 113 bytes: an eight-byte header plus one 105-byte field, tagged with the catalog version.
    static func putEnvelopeLyingAboutItsVersion() -> [UInt8] {
        let value = [UInt8](repeating: 0x78, count: 101)
        let fields = field(0x8001, value)
        var writer = ByteWriter()
        writer.u16(ObjectKind.route.rawValue)
        writer.u8(SchemaClass.catalogProjection.version)  // the lie
        writer.u8(0)
        writer.u16(UInt16(fields.count))
        writer.u16(1)
        writer.raw(fields)
        return writer.bytes
    }
}
