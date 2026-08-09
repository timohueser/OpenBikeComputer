import Foundation
import Testing
@testable import OBCWeatherWire

/// The Swift half of the OBCG contract: this decoder is written from `specs/OBCG_Spec.md`, never
/// from the Rust authority, and these tests pin the shared vectors in `specs/vectors` to exactly
/// the cells `host/obc-vectors` pins on the Rust side.
struct OBCGridCodecTests {
    private static let vectorsDirectory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // OBCWeatherWireTests
        .deletingLastPathComponent() // Tests
        .deletingLastPathComponent() // OBCKit
        .deletingLastPathComponent() // Packages
        .deletingLastPathComponent() // companion-ios
        .deletingLastPathComponent() // repository root
        .appendingPathComponent("specs/vectors")

    /// `specs/vectors/manifest.json`'s `grid.positives` block: name, byte length, object CRC-32.
    private static let positives: [(name: String, byteLength: Int, objectCRC: UInt32)] = [
        ("grid-minimal-dry.obcg", 228, 0x2E08_A044),
        ("grid-raw-tile.obcg", 308, 0x2C3F_9164),
        ("grid-rle-tile.obcg", 196, 0x4879_AADD),
        ("grid-nodata-tile.obcg", 196, 0x06EE_C5E8),
        ("grid-multipage.obcg", 406, 0x91A4_4671),
        ("grid-edge-padding.obcg", 324, 0x065E_72F0),
    ]

    private static let negatives = [
        "grid-invalid-truncated.obcg", "grid-invalid-object-crc.obcg",
        "grid-invalid-header-crc.obcg", "grid-invalid-page-crc.obcg",
        "grid-invalid-bad-offset.obcg", "grid-invalid-overlap.obcg",
        "grid-invalid-impossible-dims.obcg", "grid-invalid-tile-edge.obcg",
        "grid-invalid-paging.obcg", "grid-invalid-rle-overlong.obcg",
        "grid-invalid-rle-noncanonical.obcg", "grid-invalid-raw-compressible.obcg",
        "grid-invalid-dry-encoded.obcg", "grid-invalid-dry-sentinel-nonzero.obcg",
            "grid-invalid-dry-sentinel-edge-tile.obcg",
        "grid-invalid-tile-crc.obcg", "grid-invalid-reserved.obcg", "grid-invalid-flags.obcg",
    ]

    private func fixture(_ name: String) throws -> Data {
        let url = Self.vectorsDirectory.appendingPathComponent(name)
        return try #require(FileManager.default.contents(atPath: url.path), "missing fixture \(name)")
    }

    /// The spec's corridor read pattern, byte for byte: header, covering page, tile payload.
    private func sample(_ bytes: Data, column: UInt32, row: UInt32) throws -> UInt8 {
        let header = try OBCGridCodec.decodeHeader(bytes)
        let location = try OBCGridCodec.locateCell(header: header, column: column, row: row)
        let page = try #require(bytes.readBytes(at: location.pageOffset, count: location.pageLength))
        let entry = try OBCGridCodec.decodeEntry(page: page, indexInPage: location.indexInPage)
        var payload = Data()
        if !entry.isDry {
            let range = try OBCGridCodec.payloadRange(header: header, entry: entry)
            payload = try #require(bytes.readBytes(at: range.lowerBound, count: range.count))
        }
        return try OBCGridCodec.cell(
            header: header, column: column, row: row, page: page, payload: payload)
    }

    @Test
    func positiveGoldenVectorsValidateAndMatchTheManifest() throws {
        for positive in Self.positives {
            let bytes = try fixture(positive.name)
            #expect(bytes.count == positive.byteLength, "byte length drift for \(positive.name)")
            let header = try OBCGridCodec.validate(bytes)
            #expect(Int(header.totalLength) == bytes.count)
            #expect(header.objectCRC32 == positive.objectCRC, "object CRC drift for \(positive.name)")
            #expect(header.productID != 0)
            #expect(header.tier != 0)
            #expect(header.validAtUnixSeconds >= header.referenceTimeUnixSeconds)
        }
    }

    @Test
    func pinnedCellsAgreeWithTheRustByteAuthority() throws {
        let multipage = try fixture("grid-multipage.obcg")
        #expect(try sample(multipage, column: 0, row: 0) == 6)
        #expect(try sample(multipage, column: 39, row: 39) == 9)
        #expect(try sample(multipage, column: 20, row: 20) == 0, "dry sentinel decodes as dry")

        let raw = try fixture("grid-raw-tile.obcg")
        #expect(try sample(raw, column: 0, row: 0) == 0)
        #expect(try sample(raw, column: 12, row: 0) == 12)
        #expect(try sample(raw, column: 5, row: 3) == UInt8((3 * 16 + 5) % 13))

        let nodata = try fixture("grid-nodata-tile.obcg")
        #expect(try sample(nodata, column: 8, row: 8) == 15, "no-data is never dry")

        let padding = try fixture("grid-edge-padding.obcg")
        #expect(try sample(padding, column: 3, row: 4) == 2)
        #expect(try sample(padding, column: 20, row: 20) == 12)
    }

    @Test
    func theAllDryObjectCarriesOneSentinelAndNoPayloadBytes() throws {
        let bytes = try fixture("grid-minimal-dry.obcg")
        let header = try OBCGridCodec.validate(bytes)
        #expect(header.dataLength == 0)
        #expect(header.tileCount == 1)
        #expect(header.pageCount == 1)
        #expect(bytes.count == OBCGridCodec.headerLength + header.pageBytes)

        let pageOffset = try #require(header.pageOffset(0))
        let page = try #require(bytes.readBytes(at: pageOffset, count: header.pageBytes))
        let entry = try OBCGridCodec.decodeEntry(page: page, indexInPage: 0)
        #expect(entry.isDry)
        #expect(entry == OBCGridTileEntry(dataOffset: 0, encodedLength: 0, codec: 0, crc32: 0))
        let cells = try OBCGridCodec.decodeTileCells(header: header, entry: entry, payload: Data())
        #expect(cells.count == header.tileCells)
        #expect(cells.allSatisfy { $0 == 0 })
        #expect(try sample(bytes, column: 31, row: 31) == 0)
    }

    @Test
    func malformedGoldenVectorsAreRejected() throws {
        for name in Self.negatives {
            let bytes = try fixture(name)
            #expect(throws: (any Error).self, "accepted \(name)") { try OBCGridCodec.validate(bytes) }
        }
        #expect(Self.negatives.count == 18)
    }

    /// OBCG_Spec.md §7: a corridor consumer reads the header, the covering directory pages, and
    /// only the non-dry tiles it needs. This counts the simulated Range requests exactly.
    @Test
    func corridorExtractionTouchesOnlyHeaderPagesAndNeededTiles() throws {
        struct Request: Equatable { var offset: Int; var length: Int }

        let bytes = try fixture("grid-multipage.obcg")
        var requests: [Request] = []
        func read(_ offset: Int, _ length: Int) throws -> Data {
            requests.append(Request(offset: offset, length: length))
            return try #require(bytes.readBytes(at: offset, count: length))
        }

        let header = try OBCGridCodec.decodeHeader(read(0, OBCGridCodec.headerLength))
        let minimum = try #require(header.tileOfCell(column: 20, row: 20))
        let maximum = try #require(header.tileOfCell(column: 39, row: 39))
        #expect(minimum == OBCGridTileCoordinate(column: 1, row: 1))
        #expect(maximum == OBCGridTileCoordinate(column: 2, row: 2))

        // Covering pages, computed from the header alone, fetched once each.
        var pages: [Int] = []
        var tiles: [Int] = []
        for tileRow in minimum.row...maximum.row {
            for tileColumn in minimum.column...maximum.column {
                let index = try #require(
                    header.tileIndex(OBCGridTileCoordinate(column: tileColumn, row: tileRow)))
                tiles.append(index)
                let page = header.pageOfEntry(index)
                if !pages.contains(page) { pages.append(page) }
            }
        }
        #expect(tiles == [4, 5, 7, 8])
        #expect(pages == [2, 3, 4], "tiles 4, 5, 7 and 8 at two entries per page")

        var wetCells = 0
        for page in pages {
            let pageOffset = try #require(header.pageOffset(page))
            let pageBytes = try read(pageOffset, header.pageBytes)
            try OBCGridCodec.validatePage(header: header, page: pageBytes)
            for index in tiles where header.pageOfEntry(index) == page {
                let within = index - page * Int(header.entriesPerPage)
                let entry = try OBCGridCodec.decodeEntry(page: pageBytes, indexInPage: within)
                if entry.isDry { continue } // a dry tile costs no tile read
                let range = try OBCGridCodec.payloadRange(header: header, entry: entry)
                let payload = try read(range.lowerBound, range.count)
                let cells = try OBCGridCodec.decodeTileCells(
                    header: header, entry: entry, payload: payload)
                wetCells += cells.filter { $0 != 0 && $0 != 15 }.count
            }
        }
        #expect(wetCells == 1, "exactly the north-east corner cell")

        // The complete ledger: one header read, three covering page reads, and one tile read per
        // non-dry needed tile — tiles 5 and 7 are no-data-padded edge tiles and tile 8 holds the
        // wet corner, while interior tile 4 is a dry sentinel and costs nothing.
        #expect(requests.count == 7, "request ledger: \(requests)")
        #expect(requests[0] == Request(offset: 0, length: OBCGridCodec.headerLength))
        let pageRequests = try pages.map { page in
            Request(offset: try #require(header.pageOffset(page)), length: header.pageBytes)
        }
        var tileReads = 0
        for request in requests.dropFirst() where !pageRequests.contains(request) {
            #expect(request.offset >= Int(header.dataOffset), "tile read lies in the data section")
            #expect(request.offset + request.length <= Int(header.totalLength))
            tileReads += 1
        }
        #expect(tileReads == 3)
    }

    @Test
    func headerOnlyReadsProveTheirOwnIntegrityAndTheCellLattice() throws {
        let bytes = try fixture("grid-multipage.obcg")
        let header = try OBCGridCodec.decodeHeader(bytes.prefix(OBCGridCodec.headerLength))
        #expect(header.width == 40)
        #expect(header.height == 40)
        #expect(header.tileEdge == 16)
        #expect(header.entriesPerPage == 2)
        #expect(header.tileColumns == 3)
        #expect(header.tileRows == 3)
        #expect(header.tileCount == 9)
        #expect(header.pageCount == 5)
        #expect(header.pageBytes == 28)
        #expect(Int(header.dataOffset) == OBCGridCodec.headerLength + 5 * 28)
        #expect(header.northLatitudeMicrodegrees
            == Int64(header.southLatitudeMicrodegrees) + 40 * Int64(header.cellLatitudeStrideMicrodegrees))

        // §6 lookup: the south-west corner is cell (0, 0); the north and east edges are outside.
        let south = Int64(header.southLatitudeMicrodegrees), west = Int64(header.westLongitudeMicrodegrees)
        let cell = try #require(header.cellOfCoordinate(latitudeMicrodegrees: south, longitudeMicrodegrees: west))
        #expect(cell == OBCGridCellCoordinate(column: 0, row: 0))
        #expect(header.cellOfCoordinate(
            latitudeMicrodegrees: header.northLatitudeMicrodegrees, longitudeMicrodegrees: west) == nil)
        #expect(header.cellOfCoordinate(
            latitudeMicrodegrees: south, longitudeMicrodegrees: header.eastLongitudeMicrodegrees) == nil)
        #expect(header.tileOfCell(column: 40, row: 0) == nil)

        // A header read whose CRC no longer matches is refused before any derived arithmetic.
        var tampered = Data(bytes.prefix(OBCGridCodec.headerLength))
        tampered[13] ^= 0x01
        #expect(throws: OBCWeatherWireError.crcMismatch) { try OBCGridCodec.decodeHeader(tampered) }
    }

    @Test
    func everyTruncationAndArbitraryBytesFailClosed() throws {
        let bytes = try fixture("grid-multipage.obcg")
        for length in 0..<bytes.count {
            #expect((try? OBCGridCodec.validate(Data(bytes.prefix(length)))) == nil)
        }

        var state: UInt32 = 0x0BC6_1190
        for length in 0..<512 {
            var random = Data(); random.reserveCapacity(length)
            for _ in 0..<length {
                state ^= state << 13; state ^= state >> 17; state ^= state << 5
                random.append(UInt8(truncatingIfNeeded: state))
            }
            _ = try? OBCGridCodec.validate(random)
            _ = try? OBCGridCodec.decodeHeader(random)
        }
    }

    @Test
    func theGeneralizedTileCodecKeepsThe256CellPathAndCanonicalityRules() throws {
        // OBCG's per-product tile sizes run through the same authority as OBCW's fixed tile.
        for cellCount in [256, 1_024, 4_096] {
            let raw = (0..<cellCount).map { UInt8($0 % 13) }
            let uniform = [UInt8](repeating: 6, count: cellCount)
            for (cells, codec, length) in [
                (raw, OBCPrecipitationTileCodec.raw4, cellCount / 2),
                (uniform, OBCPrecipitationTileCodec.rle4, cellCount / 16),
            ] {
                let encoded = try OBCPrecipitationTileCodec.encodeCells(cells)
                #expect(encoded.codec == codec)
                #expect(encoded.bytes.count == length)
                #expect(try OBCPrecipitationTileCodec.decodeCells(
                    codec: encoded.codec, encoded: encoded.bytes, cellCount: cellCount) == cells)
            }
        }

        // The 256-cell entry points stay exact wrappers.
        let tile = (0..<256).map { UInt8($0 % 13) }
        #expect(try OBCPrecipitationTileCodec.encode(tile).bytes
            == OBCPrecipitationTileCodec.encodeCells(tile).bytes)
        #expect(try OBCPrecipitationTileCodec.encodedLength(tile) == 128)
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.encode([UInt8](repeating: 0, count: 1_024))
        }

        // A payload is never valid against a different declared cell count, and the §5 rules hold
        // at every tile size: compressible raw4, overlong RLE, and split equal runs are refused.
        let compact = try OBCPrecipitationTileCodec.encodeCells([UInt8](repeating: 6, count: 256))
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.validateCells(
                codec: compact.codec, encoded: compact.bytes, cellCount: 1_024)
        }
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.validateCells(
                codec: 0, encoded: Data(repeating: 0xFF, count: 512), cellCount: 1_024)
        }
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.validateCells(
                codec: 1, encoded: Data(repeating: 0xF6, count: 65), cellCount: 1_024)
        }
        #expect(throws: OBCWeatherWireError.malformed) {
            try OBCPrecipitationTileCodec.validateCells(
                codec: 1, encoded: Data([0xE6, 0x06] + [UInt8](repeating: 0xF6, count: 63)),
                cellCount: 1_024)
        }
        #expect(!OBCPrecipitationTileCodec.validCellCount(OBCPrecipitationTileCodec.maximumCells + 2))
        #expect(OBCPrecipitationTileCodec.validCellCount(OBCPrecipitationTileCodec.maximumCells))
    }
}
