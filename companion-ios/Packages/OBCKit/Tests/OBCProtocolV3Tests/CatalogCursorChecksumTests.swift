import Foundation
import Testing

@testable import OBCProtocolV3

/// §8.2's cursor CRC-32 "binds a cursor to one store": it covers the StoreId that minted the cursor
/// followed by the cursor's own first twelve bytes.
///
/// Where that check can be *made* is asymmetric, and this suite pins both halves. A catalog page
/// reports the StoreId it was minted under, so the page's own next cursor is verifiable at decode
/// time and a corrupted or foreign one is `checksumFailure/cursor` rather than something to follow.
/// A QueryCatalog *request* carries no StoreId at all, so a decoder holding only the frame has
/// nothing to verify against and must not invent one — the device checks it against the store it
/// actually has.
@Suite("Device Object v3 — catalog cursor CRC")
struct CatalogCursorChecksumTests {
    static let storeId: StoreId = {
        guard let id = StoreId(bytes: [UInt8](repeating: 0x3C, count: 16)) else {
            fatalError("16 bytes is a StoreId")
        }
        return id
    }()
    static let otherStoreId: StoreId = {
        guard let id = StoreId(bytes: [UInt8](repeating: 0x7E, count: 16)) else {
            fatalError("16 bytes is a StoreId")
        }
        return id
    }()

    static let pageRevision: UInt64 = 41

    /// A route catalog projection with both required fields, so the page's entry validates.
    static let projection = MetadataEnvelope(
        schemaId: ObjectKind.route.rawValue, schemaVersion: SchemaClass.catalogProjection.version,
        fields: [
            MetadataField(baseTag: 0x0001, critical: true, value: Array("loop".utf8)),
            MetadataField(baseTag: 0x0002, critical: true, value: [2]),
        ])

    /// The sixteen cursor bytes, with a CRC computed under `crcStoreId` — which the callers below
    /// vary independently of the store the page reports.
    static func cursor(crcStoreId: StoreId, corruptCRC: Bool = false) -> CatalogCursor {
        var writer = ByteWriter()
        writer.u64(pageRevision)
        writer.u16(1)  // next entry index
        writer.u16(ObjectKind.route.rawValue)
        let leading = writer.bytes
        var crc = CRC32IEEE.checksum(crcStoreId.bytes + leading)
        if corruptCRC { crc ^= 1 }
        writer.u32(crc)
        guard let cursor = CatalogCursor(bytes: writer.bytes) else {
            fatalError("the cursor is sixteen bytes by construction")
        }
        return cursor
    }

    static func page(_ cursor: CatalogCursor) throws -> [UInt8] {
        try CatalogPage(
            storeId: storeId, objectKind: .route, repositoryRevision: Revision(pageRevision),
            nextCursor: cursor,
            entries: [
                CatalogEntry(
                    logicalObjectId: LogicalObjectId(9), revision: Revision(pageRevision),
                    length: 3000, crc32: 0x4636_A985, metadata: projection)
            ]
        ).encoded()
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

    @Test("a page whose next cursor reproduces under its own StoreId decodes")
    func matchingCursorIsFollowed() throws {
        let bytes = try Self.page(Self.cursor(crcStoreId: Self.storeId))
        let page = try CatalogPage.decode(bytes, more: true)
        #expect(page.nextCursor.checksum == page.nextCursor.expectedChecksum(storeId: Self.storeId))
        #expect(page.entries.count == 1)
    }

    @Test("one flipped CRC bit is checksumFailure/cursor")
    func corruptedCursorIsRefused() throws {
        let bytes = try Self.page(Self.cursor(crcStoreId: Self.storeId, corruptCRC: true))
        let fault = Self.fault { _ = try CatalogPage.decode(bytes, more: true) }
        #expect(fault?.category == .checksumFailure)
        #expect(fault?.detailName == "cursor")
    }

    /// The binding is to a *store*, not merely to the twelve bytes: a cursor minted elsewhere is
    /// well-formed and still refused on this page.
    @Test("a cursor minted under another StoreId is refused")
    func foreignCursorIsRefused() throws {
        let bytes = try Self.page(Self.cursor(crcStoreId: Self.otherStoreId))
        let fault = Self.fault { _ = try CatalogPage.decode(bytes, more: true) }
        #expect(fault?.category == .checksumFailure)
        #expect(fault?.detailName == "cursor")
    }

    /// The other half of the asymmetry: the same foreign cursor in a *request* decodes, because the
    /// request frame does not carry the store the cursor was scoped to.
    @Test("a request cursor is not CRC-checked by the codec")
    func requestCursorIsNotVerified() throws {
        let request = QueryCatalogRequest(
            objectKind: .route, flags: [.expectedRevision, .cursor],
            expectedRevision: Revision(Self.pageRevision),
            cursor: Self.cursor(crcStoreId: Self.otherStoreId))
        let decoded = try QueryCatalogRequest.decode(try request.encoded())
        #expect(decoded == request)
    }
}
