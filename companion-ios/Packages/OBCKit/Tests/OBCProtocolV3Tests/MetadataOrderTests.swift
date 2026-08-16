import Foundation
import Testing

@testable import OBCProtocolV3

/// §2.2's frozen validation order: canonical form, then the schema's field rules, then the per-kind
/// registered maximum **last**, so an envelope failing more than one rule reports one deterministic
/// error.
///
/// The checked-in vectors pin the second boundary with a pair of catalog projections; these are the
/// same decisions taken directly against the envelope codec, plus the sub-ordering *inside* the
/// canonical-form stage that the frozen text leaves to the fixtures.
@Suite("Device Object v3 — metadata validation order")
struct MetadataOrderTests {
    /// Builds a ride catalog projection: the four required fields, plus whatever extra fields the
    /// caller wants appended.
    static func rideProjection(extra: [(tag: UInt16, value: [UInt8])]) -> [UInt8] {
        var fields = ByteWriter()
        func field(_ tag: UInt16, _ value: [UInt8]) {
            fields.u16(tag)
            fields.u16(UInt16(value.count))
            fields.raw(value)
        }
        field(0x8001, [0x00, 0xF1, 0x53, 0x65, 0, 0, 0, 0])  // start UTC, i64
        field(0x8002, [0x18, 0x15, 0, 0])  // duration, u32
        field(0x8003, [0x10, 0xA4, 0, 0])  // distance, u32
        field(0x8004, [0x01])  // imported acknowledgement, boolean
        var count = 4
        for entry in extra {
            field(entry.tag, entry.value)
            count += 1
        }
        var writer = ByteWriter()
        writer.u16(ObjectKind.ride.rawValue)
        writer.u8(SchemaClass.catalogProjection.version)
        writer.u8(0)
        writer.u16(UInt16(fields.bytes.count))
        writer.u16(UInt16(count))
        writer.raw(fields.bytes)
        return writer.bytes
    }

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

    /// Ride's registered catalog maximum is 41 bytes. Both envelopes below exceed it, and the order
    /// is what decides which error each reports.
    @Test("an unknown critical field in an oversized envelope reports the field, not the size")
    func fieldRulesBeforeTheRegisteredMaximum() throws {
        let bytes = Self.rideProjection(extra: [(tag: 0x8055, value: [0x07])])
        #expect(bytes.count > 41)
        let envelope = try MetadataEnvelope.decode(bytes, maximumEncodedLength: SchemaClass.catalogProjection.envelopeCeiling)
        let fault = Self.fault {
            try envelope.validated(kind: .ride, schemaClass: .catalogProjection, mutating: false)
        }
        #expect(fault?.category == .invalidDescriptor)
        #expect(fault?.detailName == "invalidCombination")
    }

    /// The same envelope with the extra field *noncritical*: a projection may skip it, so the only
    /// rule left to fail is the size, and the error becomes the nested-length one.
    @Test("a skippable unknown field leaves only the size to fail")
    func registeredMaximumAfterSkippableFields() throws {
        let bytes = Self.rideProjection(
            extra: [(tag: 0x0055, value: [UInt8](repeating: 0x33, count: 50))])
        #expect(bytes.count > 41)
        let envelope = try MetadataEnvelope.decode(bytes, maximumEncodedLength: SchemaClass.catalogProjection.envelopeCeiling)
        let fault = Self.fault {
            try envelope.validated(kind: .ride, schemaClass: .catalogProjection, mutating: false)
        }
        #expect(fault?.category == .invalidDescriptor)
        #expect(fault?.detailName == "nestedLength")
    }

    /// The same skippable field, small enough to stay under ride's 41-byte maximum, decodes: a
    /// projection reader really may skip a well-formed unknown noncritical field.
    @Test("a skippable unknown field inside the registered maximum is accepted")
    func skippableFieldIsSkipped() throws {
        // Ride's four required fields already occupy 41 bytes, so use route — 82 — to leave room.
        var fields = ByteWriter()
        func field(_ tag: UInt16, _ value: [UInt8]) {
            fields.u16(tag)
            fields.u16(UInt16(value.count))
            fields.raw(value)
        }
        field(0x8001, Array("loop".utf8))
        field(0x8002, [0x02])
        field(0x0055, [0x09])  // unknown, noncritical
        var writer = ByteWriter()
        writer.u16(ObjectKind.route.rawValue)
        writer.u8(SchemaClass.catalogProjection.version)
        writer.u8(0)
        writer.u16(UInt16(fields.bytes.count))
        writer.u16(3)
        writer.raw(fields.bytes)

        let envelope = try MetadataEnvelope.decode(writer.bytes, maximumEncodedLength: SchemaClass.catalogProjection.envelopeCeiling)
        let decoded = try envelope.validated(
            kind: .route, schemaClass: .catalogProjection, mutating: false)
        #expect(decoded[0x0001] == .text("loop"))
        #expect(decoded[0x0055] == nil)
        // The same envelope in a mutating request rejects the unknown field outright.
        let rejected = Self.fault {
            try envelope.validated(kind: .route, schemaClass: .catalogProjection, mutating: true)
        }
        #expect(rejected?.detailName == "invalidCombination")
    }

    /// The sub-ordering *inside* the canonical-form stage, which the frozen paragraph does not
    /// resolve: the class ceiling (120 Put/patch field bytes, 88 catalog) is checked before the
    /// declared length is compared against the bytes actually present. Otherwise an envelope that
    /// merely *claims* to exceed its ceiling reads as truncation rather than as a nested-length
    /// error — which is what the `metadata-above-the-catalog-ceiling` vector requires.
    @Test("the class ceiling is checked before the declared body length")
    func classCeilingBeforeBodyLength() {
        var writer = ByteWriter()
        writer.u16(ObjectKind.route.rawValue)
        writer.u8(SchemaClass.catalogProjection.version)
        writer.u8(0)
        writer.u16(89)  // one over the 88-byte catalog field-body ceiling
        writer.u16(0)
        let fault = Self.fault { _ = try MetadataEnvelope.decode(writer.bytes, maximumEncodedLength: SchemaClass.catalogProjection.envelopeCeiling) }
        #expect(fault?.category == .invalidDescriptor)
        #expect(fault?.detailName == "nestedLength")
    }

    /// A duplicate base tag is reported as itself. This envelope carries a *registered* version, so
    /// it pins the plain canonical-form path only — the interesting case, where canonical form must
    /// win against a competing schema fault, needs a doubly-invalid envelope and lives in
    /// `MetadataOrderDivergenceTests`, which is where the four such pairs are exercised.
    @Test("a duplicate base tag is reported as itself")
    func duplicateBaseTagIsReported() {
        var fields = ByteWriter()
        fields.u16(0x8001)
        fields.u16(1)
        fields.u8(0x02)
        fields.u16(0x8001)
        fields.u16(1)
        fields.u8(0x02)
        var writer = ByteWriter()
        writer.u16(ObjectKind.route.rawValue)
        writer.u8(SchemaClass.patch.version)
        writer.u8(0)
        writer.u16(UInt16(fields.bytes.count))
        writer.u16(2)
        writer.raw(fields.bytes)
        let fault = Self.fault { _ = try MetadataEnvelope.decode(writer.bytes, maximumEncodedLength: SchemaClass.catalogProjection.envelopeCeiling) }
        #expect(fault?.detailName == "duplicateField")
    }
}
