import Foundation
import Testing

@testable import OBCProtocolV3

/// Registries §3.1's weather Put envelope declares the **validated** form of exactly the facts §8.4
/// reports back in the durable request context, so the same bounds bind both directions. A codec
/// that range-checks the response and not the Put would accept a bundle declaring a coverage centre
/// no request context could ever have asked for.
///
/// Detail convention, shared across the three languages: a **continuous** quantity out of bounds is
/// `invalidDescriptor/invalidCombination`; a value that is not a member of an **enumerated** set is
/// `invalidDescriptor/unknownEnum`; and anything §2.2's *encoding* paragraph governs — a wrong
/// registered width, a boolean byte that is neither `0` nor `1`, text that is not clean UTF-8 — is
/// `invalidDescriptor/noncanonicalMetadata`, because those are rules about the encoding rather than
/// about the registered value space. The last of the three is pinned at the bottom of this suite:
/// no checked-in fixture exercises it, and the first draft of this codec answered `unknownEnum` for
/// a bad boolean, which diverged from the Rust codec.
@Suite("Device Object v3 — weather Put ranges")
struct WeatherPutRangeTests {
    static func envelope(
        requestId: UInt64 = 42, latitude: Int32 = 480_000_000, longitude: Int32 = 77_000_000,
        radius: UInt32 = 50_000, issued: Int64 = 1_700_000_000, validUntil: Int64 = 1_700_086_400
    ) -> [UInt8] {
        var fields = ByteWriter()
        func field(_ tag: UInt16, _ value: [UInt8]) {
            fields.u16(tag)
            fields.u16(UInt16(value.count))
            fields.raw(value)
        }
        var w = ByteWriter()
        w.u64(requestId)
        field(0x8001, w.bytes)
        w = ByteWriter()
        w.i32(latitude)
        field(0x8002, w.bytes)
        w = ByteWriter()
        w.i32(longitude)
        field(0x8003, w.bytes)
        w = ByteWriter()
        w.u32(radius)
        field(0x8004, w.bytes)
        w = ByteWriter()
        w.i64(issued)
        field(0x8005, w.bytes)
        w = ByteWriter()
        w.i64(validUntil)
        field(0x8006, w.bytes)

        var writer = ByteWriter()
        writer.u16(ObjectKind.weather.rawValue)
        writer.u8(SchemaClass.put.version)
        writer.u8(0)
        writer.u16(UInt16(fields.bytes.count))
        writer.u16(6)
        writer.raw(fields.bytes)
        return writer.bytes
    }

    static func validate(_ bytes: [UInt8]) throws {
        let envelope = try MetadataEnvelope.decode(
            bytes, maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        try envelope.validated(kind: .weather, schemaClass: .put, mutating: true)
    }

    static func fault(_ bytes: [UInt8]) -> WireFault? {
        do {
            try validate(bytes)
            return nil
        } catch let fault as WireFault {
            return fault
        } catch {
            return nil
        }
    }

    /// Registries §4's published maximum is 68 bytes, and this is that envelope.
    @Test("the canonical envelope validates and is the registered size")
    func canonicalEnvelope() throws {
        let bytes = Self.envelope()
        #expect(bytes.count == 68)
        try Self.validate(bytes)
    }

    // MARK: latitude, ±900,000,000

    @Test("latitude accepts both bounds", arguments: [Int32(-900_000_000), 900_000_000, 0])
    func latitudeInRange(_ latitude: Int32) throws {
        try Self.validate(Self.envelope(latitude: latitude))
    }

    @Test("latitude rejects one past both bounds", arguments: [Int32(-900_000_001), 900_000_001])
    func latitudeOutOfRange(_ latitude: Int32) {
        let fault = Self.fault(Self.envelope(latitude: latitude))
        #expect(fault?.category == .invalidDescriptor)
        #expect(fault?.detailName == "invalidCombination")
    }

    // MARK: longitude, ±1,800,000,000

    @Test("longitude accepts both bounds", arguments: [Int32(-1_800_000_000), 1_800_000_000, 0])
    func longitudeInRange(_ longitude: Int32) throws {
        try Self.validate(Self.envelope(longitude: longitude))
    }

    @Test(
        "longitude rejects one past both bounds", arguments: [Int32(-1_800_000_001), 1_800_000_001])
    func longitudeOutOfRange(_ longitude: Int32) {
        let fault = Self.fault(Self.envelope(longitude: longitude))
        #expect(fault?.detailName == "invalidCombination")
    }

    // MARK: radius, nonzero through 100,000

    @Test("radius accepts both bounds", arguments: [UInt32(1), 100_000])
    func radiusInRange(_ radius: UInt32) throws {
        try Self.validate(Self.envelope(radius: radius))
    }

    @Test("radius rejects zero and one past the maximum", arguments: [UInt32(0), 100_001])
    func radiusOutOfRange(_ radius: UInt32) {
        let fault = Self.fault(Self.envelope(radius: radius))
        #expect(fault?.detailName == "invalidCombination")
    }

    // MARK: valid-until strictly later than issued

    @Test("valid-until one second later is accepted")
    func validityJustLater() throws {
        try Self.validate(Self.envelope(issued: 1_700_000_000, validUntil: 1_700_000_001))
    }

    @Test("valid-until equal to or earlier than issued is refused")
    func validityNotLater() {
        for validUntil: Int64 in [1_700_000_000, 1_699_999_999] {
            let fault = Self.fault(Self.envelope(issued: 1_700_000_000, validUntil: validUntil))
            #expect(fault?.detailName == "invalidCombination")
        }
    }

    /// The same bounds on the §8.4 response side, so the two directions cannot drift apart.
    @Test("the request-context response enforces the identical bounds")
    func responseSideAgrees() {
        func context(latitude: Int32 = 0, longitude: Int32 = 0, radius: UInt32 = 50_000,
            issued: Int64 = 1_700_000_000, validUntil: Int64 = 1_700_086_400) -> [UInt8]
        {
            var w = ByteWriter()
            w.raw([UInt8](repeating: 0x3C, count: 16))
            w.u64(42)
            w.u64(3)
            w.u32(0)
            w.u64(0)
            w.u64(88)
            w.u64(0)
            w.i32(latitude)
            w.i32(longitude)
            w.u32(radius)
            w.i64(issued)
            w.i64(validUntil)
            w.u8(WeatherRequestContext.State.pending.rawValue)
            w.zeros(7)
            return w.bytes
        }
        for bad in [
            context(latitude: 900_000_001), context(longitude: -1_800_000_001),
            context(radius: 0), context(radius: 100_001),
            context(issued: 10, validUntil: 10),
        ] {
            #expect(throws: WireFault.self) { _ = try WeatherRequestContext.decode(bad) }
        }
        #expect(throws: Never.self) {
            _ = try WeatherRequestContext.decode(
                context(latitude: 900_000_000, longitude: 1_800_000_000, radius: 100_000))
        }
    }

    /// The enumerated half of the convention, for contrast: route retention (`0…5`) and update
    /// state (`1…6`) are sets, not intervals, so they report `unknownEnum`.
    @Test("enumerated domains report unknownEnum, not invalidCombination")
    func enumeratedDomains() throws {
        func routePut(retention: UInt8) -> [UInt8] {
            var writer = ByteWriter()
            writer.u16(ObjectKind.route.rawValue)
            writer.u8(SchemaClass.put.version)
            writer.u8(0)
            writer.u16(5)
            writer.u16(1)
            writer.u16(0x8001)
            writer.u16(1)
            writer.u8(retention)
            return writer.bytes
        }
        for accepted: UInt8 in [0, 5] {
            let envelope = try MetadataEnvelope.decode(
                routePut(retention: accepted), maximumEncodedLength: SchemaClass.put.envelopeCeiling)
            try envelope.validated(kind: .route, schemaClass: .put, mutating: true)
        }
        let envelope = try MetadataEnvelope.decode(
            routePut(retention: 6), maximumEncodedLength: SchemaClass.put.envelopeCeiling)
        do {
            try envelope.validated(kind: .route, schemaClass: .put, mutating: true)
            Issue.record("retention 6 was accepted")
        } catch let fault as WireFault {
            #expect(fault.detailName == "unknownEnum")
        }
    }

    /// The third arm of the convention. A route patch carries a boolean at tag `2` and text at tag
    /// `3`; a byte of `2` in the boolean, a control scalar in the text, and a two-byte value where
    /// the registry fixes one are all faults of *encoding*, so all three answer
    /// `noncanonicalMetadata` — not `unknownEnum`, which would claim the value space had a registered
    /// set this value is missing from.
    @Test("encoding rules report noncanonicalMetadata")
    func encodingRulesAreNoncanonical() throws {
        func routePatch(_ tag: UInt16, _ value: [UInt8]) -> [UInt8] {
            var writer = ByteWriter()
            writer.u16(ObjectKind.route.rawValue)
            writer.u8(SchemaClass.patch.version)
            writer.u8(0)
            writer.u16(UInt16(4 + value.count))
            writer.u16(1)
            writer.u16(tag)
            writer.u16(UInt16(value.count))
            writer.raw(value)
            return writer.bytes
        }
        for bytes in [
            routePatch(0x8002, [2]),  // boolean byte outside 0…1
            routePatch(0x8003, [0x6C, 0x07, 0x6F]),  // BEL inside the display name
            routePatch(0x8001, [1, 0]),  // two bytes for a one-byte retention
        ] {
            let envelope = try MetadataEnvelope.decode(
                bytes, maximumEncodedLength: SchemaClass.patch.envelopeCeiling)
            do {
                try envelope.validated(kind: .route, schemaClass: .patch, mutating: true)
                Issue.record("\(bytes.count)-byte patch was accepted")
            } catch let fault as WireFault {
                #expect(fault.category == .invalidDescriptor)
                #expect(fault.detailName == "noncanonicalMetadata")
            }
        }
    }
}
