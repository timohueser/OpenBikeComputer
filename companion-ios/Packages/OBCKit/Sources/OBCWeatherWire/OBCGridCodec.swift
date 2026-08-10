import Compression
import Foundation

/// Decoded OBCG v1 fixed header.
///
/// Every derived count below is checked arithmetic over these fields, so a corridor consumer
/// computes directory-page and tile byte ranges from the 128 header bytes alone. Product id and
/// tier are provenance only — nothing branches on either, staleness and attribution are manifest
/// data, and an unknown nonzero value in either is never a rejection reason.
public struct OBCGridHeader: Equatable, Sendable {
    public var totalLength: UInt32
    public var productID: UInt8
    public var tier: UInt8
    public var flags: UInt16
    /// Real upstream UTC frame validity timestamp; never a re-stamped fetch or bake time.
    public var validAtUnixSeconds: Int64
    /// Upstream run/reference UTC timestamp the frame was derived from.
    public var referenceTimeUnixSeconds: Int64
    public var southLatitudeMicrodegrees: Int32
    public var westLongitudeMicrodegrees: Int32
    public var cellLatitudeStrideMicrodegrees: UInt32
    public var cellLongitudeStrideMicrodegrees: UInt32
    public var width: UInt32
    public var height: UInt32
    public var cellSizeMetres: UInt16
    public var tileEdge: UInt16
    public var entriesPerPage: UInt16
    public var dataOffset: UInt32
    public var dataLength: UInt32
    public var objectCRC32: UInt32
    public var headerCRC32: UInt32

    public init(
        totalLength: UInt32, productID: UInt8, tier: UInt8, flags: UInt16,
        validAtUnixSeconds: Int64, referenceTimeUnixSeconds: Int64,
        southLatitudeMicrodegrees: Int32, westLongitudeMicrodegrees: Int32,
        cellLatitudeStrideMicrodegrees: UInt32, cellLongitudeStrideMicrodegrees: UInt32,
        width: UInt32, height: UInt32, cellSizeMetres: UInt16, tileEdge: UInt16,
        entriesPerPage: UInt16, dataOffset: UInt32, dataLength: UInt32,
        objectCRC32: UInt32, headerCRC32: UInt32
    ) {
        self.totalLength = totalLength
        self.productID = productID
        self.tier = tier
        self.flags = flags
        self.validAtUnixSeconds = validAtUnixSeconds
        self.referenceTimeUnixSeconds = referenceTimeUnixSeconds
        self.southLatitudeMicrodegrees = southLatitudeMicrodegrees
        self.westLongitudeMicrodegrees = westLongitudeMicrodegrees
        self.cellLatitudeStrideMicrodegrees = cellLatitudeStrideMicrodegrees
        self.cellLongitudeStrideMicrodegrees = cellLongitudeStrideMicrodegrees
        self.width = width
        self.height = height
        self.cellSizeMetres = cellSizeMetres
        self.tileEdge = tileEdge
        self.entriesPerPage = entriesPerPage
        self.dataOffset = dataOffset
        self.dataLength = dataLength
        self.objectCRC32 = objectCRC32
        self.headerCRC32 = headerCRC32
    }
}

/// Tile grid coordinates; row 0 is the southernmost tile row, column 0 the westernmost.
public struct OBCGridTileCoordinate: Equatable, Sendable {
    public var column: Int
    public var row: Int

    public init(column: Int, row: Int) {
        self.column = column
        self.row = row
    }
}

/// Cell coordinates inside the grid window; row 0 is the southernmost cell row.
public struct OBCGridCellCoordinate: Equatable, Sendable {
    public var column: UInt32
    public var row: UInt32

    public init(column: UInt32, row: UInt32) {
        self.column = column
        self.row = row
    }
}

/// One decoded directory entry. `encodedLength == 0` is the all-dry sentinel: the tile has no
/// payload bytes and every other field is zero. The sentinel means dry, never no-data.
public struct OBCGridTileEntry: Equatable, Sendable {
    public var dataOffset: UInt32
    public var encodedLength: UInt16
    public var codec: UInt8
    public var crc32: UInt32

    public init(dataOffset: UInt32, encodedLength: UInt16, codec: UInt8, crc32: UInt32) {
        self.dataOffset = dataOffset
        self.encodedLength = encodedLength
        self.codec = codec
        self.crc32 = crc32
    }

    public var isDry: Bool { encodedLength == 0 }
}

/// Everything a corridor consumer needs to fetch and address one cell: which directory page
/// covers its tile, and where the cell sits inside the decoded tile.
public struct OBCGridCellLocation: Equatable, Sendable {
    public var tile: OBCGridTileCoordinate
    public var tileIndex: Int
    public var page: Int
    public var pageOffset: Int
    public var pageLength: Int
    public var indexInPage: Int
    public var cellIndexInTile: Int

    public init(
        tile: OBCGridTileCoordinate, tileIndex: Int, page: Int, pageOffset: Int, pageLength: Int,
        indexInPage: Int, cellIndexInTile: Int
    ) {
        self.tile = tile
        self.tileIndex = tileIndex
        self.page = page
        self.pageOffset = pageOffset
        self.pageLength = pageLength
        self.indexInPage = indexInPage
        self.cellIndexInTile = cellIndexInTile
    }
}

public extension OBCGridHeader {
    /// Derived north edge in microdegrees; the window is half-open `[south, north)`.
    var northLatitudeMicrodegrees: Int64 {
        saturatingAdd(Int64(southLatitudeMicrodegrees),
                      saturatingMultiply(Int64(height), Int64(cellLatitudeStrideMicrodegrees)))
    }

    /// Derived east edge in microdegrees; the window is half-open `[west, east)`.
    var eastLongitudeMicrodegrees: Int64 {
        saturatingAdd(Int64(westLongitudeMicrodegrees),
                      saturatingMultiply(Int64(width), Int64(cellLongitudeStrideMicrodegrees)))
    }

    var tileColumns: Int {
        tileEdge == 0 ? 0 : (Int(width) + Int(tileEdge) - 1) / Int(tileEdge)
    }

    var tileRows: Int {
        tileEdge == 0 ? 0 : (Int(height) + Int(tileEdge) - 1) / Int(tileEdge)
    }

    var tileCount: Int {
        Int(saturatingMultiply(Int64(tileColumns), Int64(tileRows)))
    }

    /// Decoded cell count of every tile; edge tiles are no-data padded to the full square.
    var tileCells: Int { Int(tileEdge) * Int(tileEdge) }

    /// Fixed byte length of one directory page including its trailing CRC-32.
    var pageBytes: Int {
        Int(entriesPerPage) * OBCGridCodec.directoryEntryLength + OBCGridCodec.pageCRCLength
    }

    var pageCount: Int {
        entriesPerPage == 0 ? 0 : (tileCount + Int(entriesPerPage) - 1) / Int(entriesPerPage)
    }

    func pageOfEntry(_ tileIndex: Int) -> Int {
        entriesPerPage == 0 ? 0 : tileIndex / Int(entriesPerPage)
    }

    /// Absolute byte offset of directory page `page`.
    func pageOffset(_ page: Int) -> Int? {
        guard page >= 0, page < pageCount else { return nil }
        return OBCGridCodec.headerLength + page * pageBytes
    }

    /// Absolute byte offset of `tileIndex`'s 12-byte directory entry.
    func entryOffset(_ tileIndex: Int) -> Int? {
        guard tileIndex >= 0, tileIndex < tileCount,
              let base = pageOffset(pageOfEntry(tileIndex)) else { return nil }
        let within = tileIndex - pageOfEntry(tileIndex) * Int(entriesPerPage)
        return base + within * OBCGridCodec.directoryEntryLength
    }

    /// Row-major directory index of a tile; row 0 is the southernmost tile row.
    func tileIndex(_ tile: OBCGridTileCoordinate) -> Int? {
        guard tile.column >= 0, tile.row >= 0, tile.column < tileColumns, tile.row < tileRows else {
            return nil
        }
        return tile.row * tileColumns + tile.column
    }

    /// The tile covering the in-bounds cell `(column, row)`; row 0 is the southernmost cell row.
    func tileOfCell(column: UInt32, row: UInt32) -> OBCGridTileCoordinate? {
        guard column < width, row < height, tileEdge != 0 else { return nil }
        return OBCGridTileCoordinate(column: Int(column) / Int(tileEdge), row: Int(row) / Int(tileEdge))
    }

    /// True when `tileIndex` names a partial tile at the north or east grid edge. Such a tile
    /// contains no-data padding and may therefore never be a dry sentinel (OBCG_Spec.md §4.1).
    func tileIsPartial(_ tileIndex: Int) -> Bool {
        guard tileColumns > 0, tileEdge > 0 else { return false }
        let tileColumn = tileIndex % tileColumns
        let tileRow = tileIndex / tileColumns
        return Int(width) < (tileColumn + 1) * Int(tileEdge)
            || Int(height) < (tileRow + 1) * Int(tileEdge)
    }

    /// Row-major index of cell `(column, row)` inside its no-data-padded tile.
    func cellIndexInTile(column: UInt32, row: UInt32) -> Int? {
        guard tileOfCell(column: column, row: row) != nil else { return nil }
        let edge = Int(tileEdge)
        return (Int(row) % edge) * edge + (Int(column) % edge)
    }

    /// Exact integer cell lookup for an in-bounds coordinate (OBCG_Spec.md §6). The north and
    /// east edges are outside the half-open window; nearest-neighbour only, never interpolated.
    func cellOfCoordinate(
        latitudeMicrodegrees: Int64, longitudeMicrodegrees: Int64
    ) -> OBCGridCellCoordinate? {
        guard cellLatitudeStrideMicrodegrees != 0, cellLongitudeStrideMicrodegrees != 0,
              latitudeMicrodegrees >= Int64(southLatitudeMicrodegrees),
              latitudeMicrodegrees < northLatitudeMicrodegrees,
              longitudeMicrodegrees >= Int64(westLongitudeMicrodegrees),
              longitudeMicrodegrees < eastLongitudeMicrodegrees else { return nil }
        let column = (longitudeMicrodegrees - Int64(westLongitudeMicrodegrees))
            / Int64(cellLongitudeStrideMicrodegrees)
        let row = (latitudeMicrodegrees - Int64(southLatitudeMicrodegrees))
            / Int64(cellLatitudeStrideMicrodegrees)
        guard column >= 0, column < Int64(width), row >= 0, row < Int64(height) else { return nil }
        return OBCGridCellCoordinate(column: UInt32(column), row: UInt32(row))
    }
}

/// Independent Swift implementation of `specs/OBCG_Spec.md` — a consumer only. Objects are
/// produced by `host/obc-wx-bake`; the phone never writes OBCG bytes, so there is no encoder
/// here and no second canonicality authority to drift.
///
/// Codecs 0 and 1 are delegated to `OBCPrecipitationTileCodec`, the pair OBCG shares with OBCW.
/// Codec 2 (deflate4) lives **here** and nowhere else: the device decodes OBCW, so an inflate
/// must never reach the shared tile codec, and this phone-side decoder is exactly what lets the
/// corridor be re-encoded as OBCW RLE4 for a firmware that has not changed.
public enum OBCGridCodec {
    public static let headerLength = 128
    public static let directoryEntryLength = 12
    public static let pageCRCLength = 4
    /// `(16 KiB - pageCRCLength) / directoryEntryLength`: every page fits one Range request.
    public static let maximumEntriesPerPage: UInt16 = 1_365
    public static let minimumTileEdge: UInt16 = 16
    public static let maximumTileEdge: UInt16 = 256
    public static let maximumGridDimension: UInt32 = 100_000
    /// Frame cell-count ceiling, matching the WX1 decode bound.
    public static let maximumGridCells: UInt64 = 30_000_000

    public static let flagObserved: UInt16 = 1 << 0
    public static let flagForecast: UInt16 = 1 << 1

    /// The one tier code the bakery writes (OBCG §3's registry). The radar/model/floor **ladder** is
    /// gone with product selection (#1244): there is one mosaic dataset, so there is nothing to rank
    /// and no code here may be branched on. The header field itself stays until OBCG §3/§3.1 drop it
    /// in WXR7, and a nonzero value this build has never seen is still not a rejection reason.
    public static let tierMosaic: UInt8 = 4

    /// Tile codec ids (OBCG_Spec.md §4.1/§5). 0 and 1 are the shared raw4/RLE4 pair; 2 is raw
    /// DEFLATE (RFC 1951) over the tile's raw4 nibble image and exists only in OBCG.
    public static let codecRaw4 = OBCPrecipitationTileCodec.raw4
    public static let codecRLE4 = OBCPrecipitationTileCodec.rle4
    public static let codecDeflate4: UInt8 = 2

    private static let magic = Data("OBCG".utf8)
    private static let version: UInt16 = 1
    private static let objectCRCOffset = 76
    private static let headerCRCOffset = 80

    // MARK: - Header

    /// Decode and validate the fixed header, including its own CRC. Object length, whole-object
    /// CRC and the pointed-to sections are separate reader concerns; every invariant computable
    /// from the 128 header bytes alone is enforced here.
    public static func decodeHeader(_ bytes: Data) throws -> OBCGridHeader {
        guard bytes.count >= headerLength else { throw OBCWeatherWireError.malformed }
        let head = bytes.prefix(headerLength)
        guard head.readBytes(at: 0, count: 4) == magic,
              try require(head.readUInt16LE(at: 4)) == version,
              try require(head.readUInt16LE(at: 6)) == UInt16(headerLength)
        else { throw OBCWeatherWireError.malformed }
        guard headerCRC(head) == (try require(head.readUInt32LE(at: headerCRCOffset))) else {
            throw OBCWeatherWireError.crcMismatch
        }
        guard try require(head.readUInt16LE(at: 62)) == 0, head.allZero(in: 84..<headerLength) else {
            throw OBCWeatherWireError.malformed
        }
        let header = OBCGridHeader(
            totalLength: try require(head.readUInt32LE(at: 8)),
            productID: try require(head.readUInt8(at: 12)),
            tier: try require(head.readUInt8(at: 13)),
            flags: try require(head.readUInt16LE(at: 14)),
            validAtUnixSeconds: try require(head.readInt64LE(at: 16)),
            referenceTimeUnixSeconds: try require(head.readInt64LE(at: 24)),
            southLatitudeMicrodegrees: try require(head.readInt32LE(at: 32)),
            westLongitudeMicrodegrees: try require(head.readInt32LE(at: 36)),
            cellLatitudeStrideMicrodegrees: try require(head.readUInt32LE(at: 40)),
            cellLongitudeStrideMicrodegrees: try require(head.readUInt32LE(at: 44)),
            width: try require(head.readUInt32LE(at: 48)),
            height: try require(head.readUInt32LE(at: 52)),
            cellSizeMetres: try require(head.readUInt16LE(at: 56)),
            tileEdge: try require(head.readUInt16LE(at: 58)),
            entriesPerPage: try require(head.readUInt16LE(at: 60)),
            dataOffset: try require(head.readUInt32LE(at: 68)),
            dataLength: try require(head.readUInt32LE(at: 72)),
            objectCRC32: try require(head.readUInt32LE(at: objectCRCOffset)),
            headerCRC32: try require(head.readUInt32LE(at: headerCRCOffset)))
        try validateSemantics(header)
        guard try require(head.readUInt32LE(at: 64)) == UInt32(headerLength) else {
            throw OBCWeatherWireError.malformed
        }
        return header
    }

    /// CRC-32/IEEE over the 128 header bytes with the header-CRC field treated as zero. The
    /// stored object CRC participates as written, so a header-only reader also proves that
    /// field's integrity.
    public static func headerCRC(_ headerBytes: Data) -> UInt32 {
        var hasher = CRC32.Hasher()
        hasher.update(headerBytes.prefix(headerCRCOffset))
        hasher.update([UInt8](repeating: 0, count: 4))
        hasher.update(headerBytes.prefix(headerLength).dropFirst(headerCRCOffset + 4))
        return hasher.finalize()
    }

    /// CRC-32/IEEE over the whole object with both CRC fields treated as zero.
    public static func objectCRC(_ bytes: Data) -> UInt32 {
        var hasher = CRC32.Hasher()
        hasher.update(bytes.prefix(objectCRCOffset))
        hasher.update([UInt8](repeating: 0, count: 8))
        hasher.update(bytes.dropFirst(headerCRCOffset + 4))
        return hasher.finalize()
    }

    // MARK: - Directory

    /// Verify one directory page's trailing CRC-32. `page` is the full fixed-size page, so any
    /// subset of pages verifies without bytes the consumer did not fetch.
    public static func validatePage(header: OBCGridHeader, page: Data) throws {
        guard page.count == header.pageBytes, header.pageBytes > pageCRCLength else {
            throw OBCWeatherWireError.malformed
        }
        let entryArea = page.prefix(header.pageBytes - pageCRCLength)
        let stored = try require(page.readUInt32LE(at: header.pageBytes - pageCRCLength))
        guard CRC32.checksum(entryArea) == stored else { throw OBCWeatherWireError.crcMismatch }
    }

    /// Decode one directory entry from a page's entry area. A dry sentinel must be exactly twelve
    /// zero bytes; the reserved byte is zero in every entry.
    public static func decodeEntry(page: Data, indexInPage: Int) throws -> OBCGridTileEntry {
        guard indexInPage >= 0, page.count >= pageCRCLength else { throw OBCWeatherWireError.malformed }
        let base = indexInPage * directoryEntryLength
        guard base >= 0, base + directoryEntryLength <= page.count - pageCRCLength else {
            throw OBCWeatherWireError.malformed
        }
        guard try require(page.readUInt8(at: base + 7)) == 0 else { throw OBCWeatherWireError.malformed }
        let entry = OBCGridTileEntry(
            dataOffset: try require(page.readUInt32LE(at: base)),
            encodedLength: try require(page.readUInt16LE(at: base + 4)),
            codec: try require(page.readUInt8(at: base + 6)),
            crc32: try require(page.readUInt32LE(at: base + 8)))
        if entry.isDry, entry.dataOffset != 0 || entry.codec != 0 || entry.crc32 != 0 {
            throw OBCWeatherWireError.malformed
        }
        return entry
    }

    /// Byte range of one non-dry payload, checked to lie inside the object's data section.
    public static func payloadRange(header: OBCGridHeader, entry: OBCGridTileEntry) throws -> Range<Int> {
        guard !entry.isDry else { throw OBCWeatherWireError.malformed }
        let start = Int(entry.dataOffset)
        let end = start + Int(entry.encodedLength)
        let sectionEnd = Int(header.dataOffset) + Int(header.dataLength)
        guard start >= Int(header.dataOffset), end <= sectionEnd else { throw OBCWeatherWireError.malformed }
        return start..<end
    }

    // MARK: - Tiles

    /// Verify one tile payload against its directory entry: CRC over the **stored** bytes first —
    /// so a corrupt payload never reaches a decompressor — then the §5 codec rules for this
    /// product's `tileEdge^2` cells.
    public static func validateTilePayload(
        header: OBCGridHeader, entry: OBCGridTileEntry, payload: Data
    ) throws {
        guard !entry.isDry, payload.count == Int(entry.encodedLength) else {
            throw OBCWeatherWireError.malformed
        }
        guard CRC32.checksum(payload) == entry.crc32 else { throw OBCWeatherWireError.crcMismatch }
        if entry.codec == codecDeflate4 {
            _ = try decodeDeflate4(payload: payload, cellCount: header.tileCells)
            return
        }
        try OBCPrecipitationTileCodec.validateCells(
            codec: entry.codec, encoded: payload, cellCount: header.tileCells)
    }

    /// Decode one verified tile into `tileEdge^2` row-major cells, rows advancing north. A dry
    /// entry expands to all-dry without touching payload bytes.
    public static func decodeTileCells(
        header: OBCGridHeader, entry: OBCGridTileEntry, payload: Data
    ) throws -> [UInt8] {
        guard OBCPrecipitationTileCodec.validCellCount(header.tileCells) else {
            throw OBCWeatherWireError.malformed
        }
        if entry.isDry {
            guard payload.isEmpty else { throw OBCWeatherWireError.malformed }
            return [UInt8](repeating: OBCPrecipitationTileCodec.dry, count: header.tileCells)
        }
        if entry.codec == codecDeflate4 {
            guard payload.count == Int(entry.encodedLength) else { throw OBCWeatherWireError.malformed }
            guard CRC32.checksum(payload) == entry.crc32 else { throw OBCWeatherWireError.crcMismatch }
            return try decodeDeflate4(payload: payload, cellCount: header.tileCells)
        }
        try validateTilePayload(header: header, entry: entry, payload: payload)
        return try OBCPrecipitationTileCodec.decodeCells(
            codec: entry.codec, encoded: payload, cellCount: header.tileCells)
    }

    /// OBCG_Spec.md §5 codec 2. Every rejection the Rust authority makes, in the same order:
    /// the payload must be shorter than the tile's raw4 image *before* anything is inflated; the
    /// stream must terminate, consume all of its input, and produce exactly `cellCount / 2`
    /// bytes; every nibble must be a defined intensity; and the payload must be strictly shorter
    /// than the canonical raw4/RLE4 length of the cells it decodes to.
    ///
    /// The output buffer is sized from the header, never from the payload, so an over-inflating
    /// stream is refused by construction rather than after allocating what it asked for.
    private static func decodeDeflate4(payload: Data, cellCount: Int) throws -> [UInt8] {
        guard OBCPrecipitationTileCodec.validCellCount(cellCount) else {
            throw OBCWeatherWireError.malformed
        }
        let raw4Length = cellCount / 2
        guard !payload.isEmpty, payload.count < raw4Length else { throw OBCWeatherWireError.malformed }
        guard let packed = inflateRaw(payload, exactly: raw4Length) else {
            throw OBCWeatherWireError.malformed
        }
        var cells = [UInt8](); cells.reserveCapacity(cellCount)
        for byte in packed {
            cells.append(byte & 0x0F)
            cells.append(byte >> 4)
        }
        // `encodedCellsLength` validates every intensity and returns min(raw4, maximal-run RLE4).
        let canonical = try OBCPrecipitationTileCodec.encodedCellsLength(cells)
        guard payload.count < canonical else { throw OBCWeatherWireError.malformed }
        return cells
    }

    /// Inflate one raw DEFLATE (RFC 1951) stream that must produce exactly `exactly` bytes from
    /// exactly this payload. `COMPRESSION_ZLIB` is Apple's name for the unwrapped format.
    ///
    /// The input is fed in two chunks — everything but the last byte, then the last byte with
    /// `FINALIZE` — because Apple's decoder buffers whatever it is handed and always reports
    /// `src_size == 0`, so trailing bytes are invisible in a single call. If the stream has
    /// already ended before the final byte, that byte is superfluous and the payload carries
    /// something other than one exact stream; the Rust authority reaches the same verdict from
    /// its byte-exact input count. Two calls, still one decompression pass.
    private static func inflateRaw(_ payload: Data, exactly: Int) -> [UInt8]? {
        guard payload.count >= 2 else { return nil }
        var output = [UInt8](repeating: 0, count: exactly)
        let streamPointer = UnsafeMutablePointer<compression_stream>.allocate(capacity: 1)
        defer { streamPointer.deallocate() }
        guard compression_stream_init(streamPointer, COMPRESSION_STREAM_DECODE, COMPRESSION_ZLIB)
            == COMPRESSION_STATUS_OK else { return nil }
        defer { compression_stream_destroy(streamPointer) }

        var accepted = false
        payload.withUnsafeBytes { source in
            guard let base = source.bindMemory(to: UInt8.self).baseAddress else { return }
            output.withUnsafeMutableBufferPointer { destination in
                guard let destinationBase = destination.baseAddress else { return }
                streamPointer.pointee.src_ptr = base
                streamPointer.pointee.src_size = payload.count - 1
                streamPointer.pointee.dst_ptr = destinationBase
                streamPointer.pointee.dst_size = destination.count
                // A stream that has already ended here has a trailing byte after it.
                guard compression_stream_process(streamPointer, 0) == COMPRESSION_STATUS_OK else { return }
                streamPointer.pointee.src_ptr = base + (payload.count - 1)
                streamPointer.pointee.src_size = 1
                let status = compression_stream_process(
                    streamPointer, Int32(COMPRESSION_STREAM_FINALIZE.rawValue))
                // END with the output exactly filled is the only acceptance: a truncated stream
                // never ends, an over-inflating one fills the buffer without ending, and a short
                // one leaves `dst_size` positive.
                accepted = status == COMPRESSION_STATUS_END && streamPointer.pointee.dst_size == 0
            }
        }
        return accepted ? output : nil
    }

    // MARK: - Corridor extraction

    /// Everything needed to fetch one cell: the covering directory page's byte range and the
    /// cell's index inside its tile. Computed from the header alone (OBCG_Spec.md §7 step 2).
    public static func locateCell(
        header: OBCGridHeader, column: UInt32, row: UInt32
    ) throws -> OBCGridCellLocation {
        guard let tile = header.tileOfCell(column: column, row: row),
              let tileIndex = header.tileIndex(tile),
              let cellIndex = header.cellIndexInTile(column: column, row: row)
        else { throw OBCWeatherWireError.malformed }
        let page = header.pageOfEntry(tileIndex)
        guard let pageOffset = header.pageOffset(page) else { throw OBCWeatherWireError.malformed }
        return OBCGridCellLocation(
            tile: tile, tileIndex: tileIndex, page: page, pageOffset: pageOffset,
            pageLength: header.pageBytes,
            indexInPage: tileIndex - page * Int(header.entriesPerPage), cellIndexInTile: cellIndex)
    }

    /// Decode one cell from the bytes a corridor consumer actually fetched: the covering
    /// directory page and, for a non-dry tile, that tile's payload. `payload` is empty for a dry
    /// sentinel, which costs no read.
    public static func cell(
        header: OBCGridHeader, column: UInt32, row: UInt32, page: Data, payload: Data
    ) throws -> UInt8 {
        let location = try locateCell(header: header, column: column, row: row)
        try validatePage(header: header, page: page)
        let entry = try decodeEntry(page: page, indexInPage: location.indexInPage)
        if entry.isDry {
            // §4.1: a partial edge tile can never be a dry sentinel — its padding is no-data.
            guard !header.tileIsPartial(location.tileIndex) else { throw OBCWeatherWireError.malformed }
        } else {
            _ = try payloadRange(header: header, entry: entry)
        }
        let cells = try decodeTileCells(header: header, entry: entry, payload: payload)
        guard location.cellIndexInTile < cells.count else { throw OBCWeatherWireError.malformed }
        return cells[location.cellIndexInTile]
    }

    // MARK: - Whole-object validation

    /// Full-object structural validation: the whole-object consumer's acceptance check.
    ///
    /// Order: header (with its own CRC), object length and CRC, every directory page CRC, every
    /// entry's canonical tight packing (dry sentinels all-zero, padding entries beyond the tile
    /// count all-zero), every tile payload CRC plus canonical codec, and the all-dry-sentinel
    /// canonicality and no-data edge-padding rules on every decoded tile.
    @discardableResult
    public static func validate(_ bytes: Data) throws -> OBCGridHeader {
        let header = try decodeHeader(bytes)
        guard Int(header.totalLength) == bytes.count else { throw OBCWeatherWireError.malformed }
        guard objectCRC(bytes) == header.objectCRC32 else { throw OBCWeatherWireError.crcMismatch }

        let entriesPerPage = Int(header.entriesPerPage)
        let tileCount = header.tileCount
        var cursor = Int(header.dataOffset)
        for page in 0..<header.pageCount {
            let start = headerLength + page * header.pageBytes
            guard let pageSlice = bytes.readBytes(at: start, count: header.pageBytes) else {
                throw OBCWeatherWireError.malformed
            }
            try validatePage(header: header, page: pageSlice)
            for indexInPage in 0..<entriesPerPage {
                let tileIndex = page * entriesPerPage + indexInPage
                if tileIndex >= tileCount {
                    // Padding entries beyond the tile count must be all zero.
                    let base = indexInPage * directoryEntryLength
                    guard pageSlice.allZero(in: base..<base + directoryEntryLength) else {
                        throw OBCWeatherWireError.malformed
                    }
                    continue
                }
                let entry = try decodeEntry(page: pageSlice, indexInPage: indexInPage)
                if entry.isDry {
                    // §4.1: a partial edge tile contains no-data padding and can never be a dry
                    // sentinel — accepting one would decode missing edge data as dry weather.
                    guard !header.tileIsPartial(tileIndex) else { throw OBCWeatherWireError.malformed }
                    continue
                }
                guard Int(entry.dataOffset) == cursor else { throw OBCWeatherWireError.malformed }
                let range = try payloadRange(header: header, entry: entry)
                guard let payload = bytes.readBytes(at: range.lowerBound, count: range.count) else {
                    throw OBCWeatherWireError.malformed
                }
                let cells = try decodeTileCells(header: header, entry: entry, payload: payload)
                // An all-dry tile must use the len-0 sentinel; an encoded copy is noncanonical.
                guard cells.contains(where: { $0 != OBCPrecipitationTileCodec.dry }) else {
                    throw OBCWeatherWireError.malformed
                }
                try validateTilePadding(header: header, tileIndex: tileIndex, cells: cells)
                cursor = range.upperBound
            }
        }
        guard cursor == Int(header.dataOffset) + Int(header.dataLength) else {
            throw OBCWeatherWireError.malformed
        }
        return header
    }

    // MARK: - Private

    private static func validateSemantics(_ header: OBCGridHeader) throws {
        guard header.productID != 0, header.tier != 0 else { throw OBCWeatherWireError.malformed }
        let known = flagObserved | flagForecast
        guard header.flags & ~known == 0,
              (header.flags & flagObserved != 0) != (header.flags & flagForecast != 0)
        else { throw OBCWeatherWireError.malformed }
        guard header.referenceTimeUnixSeconds > 0,
              header.validAtUnixSeconds >= header.referenceTimeUnixSeconds
        else { throw OBCWeatherWireError.malformed }
        guard header.width > 0, header.height > 0,
              header.width <= maximumGridDimension, header.height <= maximumGridDimension,
              UInt64(header.width) * UInt64(header.height) <= maximumGridCells,
              header.cellLatitudeStrideMicrodegrees > 0, header.cellLongitudeStrideMicrodegrees > 0,
              header.cellSizeMetres > 0
        else { throw OBCWeatherWireError.malformed }
        guard Int64(header.southLatitudeMicrodegrees) >= -90_000_000,
              header.northLatitudeMicrodegrees <= 90_000_000,
              Int64(header.westLongitudeMicrodegrees) >= -180_000_000,
              header.eastLongitudeMicrodegrees <= 180_000_000
        else { throw OBCWeatherWireError.malformed }
        guard header.tileEdge >= minimumTileEdge, header.tileEdge <= maximumTileEdge,
              header.tileEdge.nonzeroBitCount == 1,
              header.entriesPerPage > 0, header.entriesPerPage <= maximumEntriesPerPage
        else { throw OBCWeatherWireError.malformed }
        let directoryLength = UInt64(header.pageCount) * UInt64(header.pageBytes)
        let expectedDataOffset = UInt64(headerLength) + directoryLength
        guard UInt64(header.dataOffset) == expectedDataOffset else { throw OBCWeatherWireError.malformed }
        let expectedTotal = expectedDataOffset + UInt64(header.dataLength)
        guard expectedTotal <= UInt64(UInt32.max), UInt64(header.totalLength) == expectedTotal else {
            throw OBCWeatherWireError.malformed
        }
    }

    /// Cells outside the declared grid in an edge tile must be the no-data intensity.
    private static func validateTilePadding(
        header: OBCGridHeader, tileIndex: Int, cells: [UInt8]
    ) throws {
        let edge = Int(header.tileEdge)
        guard header.tileColumns > 0, cells.count == header.tileCells else {
            throw OBCWeatherWireError.malformed
        }
        let tileColumn = tileIndex % header.tileColumns
        let tileRow = tileIndex / header.tileColumns
        let validColumns = min(edge, max(0, Int(header.width) - tileColumn * edge))
        let validRows = min(edge, max(0, Int(header.height) - tileRow * edge))
        if validColumns == edge, validRows == edge { return }
        for localRow in 0..<edge {
            for localColumn in 0..<edge where localRow >= validRows || localColumn >= validColumns {
                guard cells[localRow * edge + localColumn] == OBCPrecipitationTileCodec.noData else {
                    throw OBCWeatherWireError.malformed
                }
            }
        }
    }

    private static func require<T>(_ value: T?) throws -> T {
        guard let value else { throw OBCWeatherWireError.malformed }
        return value
    }
}

private func saturatingMultiply(_ lhs: Int64, _ rhs: Int64) -> Int64 {
    let (value, overflow) = lhs.multipliedReportingOverflow(by: rhs)
    return overflow ? Int64.max : value
}

private func saturatingAdd(_ lhs: Int64, _ rhs: Int64) -> Int64 {
    let (value, overflow) = lhs.addingReportingOverflow(rhs)
    return overflow ? Int64.max : value
}
