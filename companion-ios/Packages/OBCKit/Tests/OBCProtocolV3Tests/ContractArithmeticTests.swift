import Foundation
import Testing

@testable import OBCProtocolV3

/// §2.2 requires the schema ceilings to be asserted **arithmetically** rather than as byte vectors,
/// because no legal envelope reaches one — a ceiling fixture would be a fixture a conforming
/// decoder must reject. These are those assertions, plus the registry maxima the schema tables in
/// `MetadataRegistry` have to reproduce exactly.
@Suite("Device Object v3 — frozen arithmetic")
struct ContractArithmeticTests {
    @Test("the 192-byte control floor is derived from the two 176-byte schema ceilings")
    func controlFloorDerivation() {
        #expect(WireLimits.catalogPagePrefixBytes == 44)
        #expect(WireLimits.catalogEntryPrefixBytes == 36)
        #expect(WireLimits.catalogMetadataCeiling == 96)
        #expect(WireLimits.catalogEntryCeilingPayload == 176)

        #expect(WireLimits.startUploadPrefixBytes == 48)
        #expect(WireLimits.metadataEnvelopeCeiling == 128)
        #expect(WireLimits.startUploadCeilingPayload == 176)

        #expect(WireLimits.catalogEntryCeilingPayload + WireLimits.controlHeaderBytes == 192)
        #expect(WireLimits.minimumControlFrame == 192)
        #expect(WireLimits.maximumControlPayload == 496)
    }

    /// §2.2's per-class field-body maxima: 120 encoded field bytes for a Put or patch envelope, 88
    /// for a catalog projection.
    @Test("the envelope class ceilings leave exactly 120 and 88 field bytes")
    func envelopeClassCeilings() {
        #expect(SchemaClass.put.envelopeCeiling - 8 == 120)
        #expect(SchemaClass.patch.envelopeCeiling - 8 == 120)
        #expect(SchemaClass.catalogProjection.envelopeCeiling - 8 == 88)
    }

    /// Registries §4's table, against the schemas this codec actually enforces. A field or bound
    /// that drifts changes one of these numbers.
    @Test("every registered schema's computed maximum matches the registry table", arguments: ObjectKind.allCases)
    func registeredMaxima(_ kind: ObjectKind) throws {
        let published = try #require(MetadataRegistry.publishedMaxima[kind])
        #expect(MetadataRegistry.maximumEncodedLength(kind: kind, schemaClass: .put) == published.put)
        #expect(
            MetadataRegistry.maximumEncodedLength(kind: kind, schemaClass: .patch) == published.patch)
        #expect(
            MetadataRegistry.maximumEncodedLength(kind: kind, schemaClass: .catalogProjection)
                == published.catalog)
        // No registered schema reaches its class ceiling; that is exactly why the ceilings are
        // asserted arithmetically instead of as fixtures.
        #expect(published.put < SchemaClass.put.envelopeCeiling)
        #expect(published.catalog < SchemaClass.catalogProjection.envelopeCeiling)
    }

    /// §2.2's two producible per-kind maxima, which *are* positive fixtures.
    @Test("the largest producible catalog entry and StartUpload are 162 and 116 payload bytes")
    func producibleMaxima() throws {
        let routeCatalog = try #require(
            MetadataRegistry.maximumEncodedLength(kind: .route, schemaClass: .catalogProjection))
        #expect(
            WireLimits.catalogPagePrefixBytes + WireLimits.catalogEntryPrefixBytes + routeCatalog
                == 162)
        let weatherPut = try #require(
            MetadataRegistry.maximumEncodedLength(kind: .weather, schemaClass: .put))
        #expect(WireLimits.startUploadPrefixBytes + weatherPut == 116)
    }

    /// §1's CRC check value. It detects accidental corruption; it is not identity, authentication,
    /// authorization, or an idempotency proof.
    @Test("CRC-32/IEEE agrees with the pinned check value")
    func crcCheckValue() {
        #expect(CRC32IEEE.checksum(Array("123456789".utf8)) == 0xCBF4_3926)
    }

    /// §11's tag, spelled out so a byte of it cannot drift silently.
    @Test("the canonical-intent tag is OBC-DOS3-INTENT plus one NUL")
    func intentTag() {
        #expect(CanonicalIntent.tag.count == 16)
        #expect(CanonicalIntent.tag.last == 0)
        #expect(String(decoding: CanonicalIntent.tag.dropLast(), as: UTF8.self) == "OBC-DOS3-INTENT")
        // §11: the two intent families cannot collide, because every local tag begins `O2-`.
        for local in CanonicalIntent.LocalTag.allCases {
            #expect(!local.bytes.starts(with: CanonicalIntent.tag.prefix(3)))
            #expect(local.bytes.starts(with: Array("O2-".utf8)))
        }
    }

    /// §12: nine registered details stay burned. A decoder still reads them — the fixture suite
    /// carries each as a decode-only row — but nothing in this codec may emit one.
    @Test("the nine reserved detail rows are registered and named")
    func reservedDetails() {
        #expect(CommonDetail.reservedInV3.count == 9)
        for pair in CommonDetail.reservedInV3 {
            #expect(CommonDetail.name(category: pair.category, code: pair.code) != nil)
        }
    }

    /// §10: ObjectResult outcome `1` is registered, reserved, and never emitted.
    @Test("the superseded-weather outcome decodes but is marked reserved")
    func reservedOutcome() {
        #expect(ObjectOutcome.reservedSupersededWeather.isReservedInV3)
        #expect(ObjectOutcome.allCases.filter(\.isReservedInV3).count == 1)
    }

    /// §1: a codec MUST decode and encode the full unsigned 64-bit range; truncating to 32 bits is
    /// nonconforming. The stream vectors cross `0xFFFF_FFFF` for exactly this reason.
    @Test("u64 offsets survive the whole range")
    func fullWidthOffsets() throws {
        let session = try #require(SessionId(17))
        for offset: UInt64 in [0, 0xFFFF_FFFE, 0xFFFF_FFFF, 0x1_0000_0000, UInt64.max - 1] {
            let frame = StreamFrame(
                sessionId: session, absoluteOffset: offset, direction: .upload, flags: [],
                body: .data([0xAB]))
            let round = try StreamFrame.decode(frame.encoded())
            #expect(round.absoluteOffset == offset)
        }
    }

    /// §1's identity model is mechanical: these types share no initializer, so the compiler is what
    /// stops a Revision from standing in for a LogicalObjectId. This test pins the one thing a type
    /// system cannot — that the two are not accidentally the same type.
    @Test("identity types are mechanically distinct")
    func distinctIdentities() {
        #expect(LogicalObjectId(7) != LogicalObjectId(8))
        #expect(SessionId(0) == nil)
        #expect(RequestId(0) == nil)
        #expect(StoreId(bytes: [1, 2, 3]) == nil)
        #expect(OperationId(bytes: [UInt8](repeating: 0, count: 16)) == OperationId.zero)
        // Distinct nominal types: `StoreId` and `OperationId` are both 16 opaque bytes and still
        // cannot be exchanged. (Uncommenting the next line must not compile.)
        // let _: StoreId = OperationId.zero
    }
}
