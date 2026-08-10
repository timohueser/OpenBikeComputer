import Foundation
@testable import OBCWeatherWire

/// A **test-only** OBCG encoder.
///
/// The shipping app is a consumer: `OBCGridCodec` has no encoder on purpose, so there is no second
/// canonicality authority to drift from `host/obc-wx-bake`. This one exists because manifest v2
/// addresses shards of a *square* 0.01° lattice, and the checked-in `specs/vectors/*.obcg` are
/// 9,000 x 14,000-microdegree grids — a shape the v2 lattice cannot express, so no combination of
/// them is a shard set. Rather than invent lattice-shaped vectors nothing else reads, the suite
/// writes its own and proves them with `OBCGridCodec.validate`, which is the same acceptance check a
/// real object faces. Every rule the validator enforces is honoured here rather than worked around:
/// tight payload packing, all-zero padding entries, the dry sentinel for an all-dry full tile, and
/// no sentinel for a partial edge tile.
enum OBCGridWriter {
    struct Spec {
        var southMicrodegrees: Int32
        var westMicrodegrees: Int32
        var cellMicrodegrees: UInt32
        var width: UInt32
        var height: UInt32
        var tileEdge: UInt16 = 16
        var entriesPerPage: UInt16 = 4
        var cellSizeMetres: UInt16 = 1_113
        var validAt: Date
        var referenceTime: Date
        var observed: Bool
        /// Row-major, rows advancing north, `width * height` canonical intensities.
        var cells: [UInt8]
    }

    static func encode(_ spec: Spec) -> Data {
        let edge = Int(spec.tileEdge)
        let tileColumns = (Int(spec.width) + edge - 1) / edge
        let tileRows = (Int(spec.height) + edge - 1) / edge
        let tileCount = tileColumns * tileRows
        let entriesPerPage = Int(spec.entriesPerPage)
        let pageBytes = entriesPerPage * OBCGridCodec.directoryEntryLength + OBCGridCodec.pageCRCLength
        let pageCount = (tileCount + entriesPerPage - 1) / entriesPerPage
        let dataOffset = OBCGridCodec.headerLength + pageCount * pageBytes

        // Payloads first: the directory points at them, and the validator insists they are packed
        // tight and in tile-index order.
        var payloads = Data()
        var entries: [(offset: UInt32, length: UInt16, codec: UInt8, crc: UInt32)?] = []
        for tileIndex in 0..<tileCount {
            let tileColumn = tileIndex % tileColumns
            let tileRow = tileIndex / tileColumns
            var tile = [UInt8](repeating: OBCPrecipitationTileCodec.noData, count: edge * edge)
            var partial = false
            for localRow in 0..<edge {
                let row = tileRow * edge + localRow
                for localColumn in 0..<edge {
                    let column = tileColumn * edge + localColumn
                    guard row < Int(spec.height), column < Int(spec.width) else {
                        partial = true
                        continue
                    }
                    tile[localRow * edge + localColumn] =
                        spec.cells[row * Int(spec.width) + column]
                }
            }
            // §4.1: the dry sentinel is the only canonical spelling of an all-dry full tile, and is
            // forbidden for a partial one because its padding is no-data rather than dry.
            if !partial, tile.allSatisfy({ $0 == OBCPrecipitationTileCodec.dry }) {
                entries.append(nil)
                continue
            }
            let encoded = try! OBCPrecipitationTileCodec.encode(tile)
            entries.append((
                offset: UInt32(dataOffset + payloads.count),
                length: UInt16(encoded.bytes.count), codec: encoded.codec,
                crc: CRC32.checksum(encoded.bytes)))
            payloads.append(encoded.bytes)
        }

        var directory = Data()
        for page in 0..<pageCount {
            var area = Data()
            for indexInPage in 0..<entriesPerPage {
                let tileIndex = page * entriesPerPage + indexInPage
                guard tileIndex < tileCount, let entry = entries[tileIndex] else {
                    area.append(Data(repeating: 0, count: OBCGridCodec.directoryEntryLength))
                    continue
                }
                area.append(littleEndian(entry.offset))
                area.append(littleEndian(entry.length))
                area.append(entry.codec)
                area.append(0)  // reserved
                area.append(littleEndian(entry.crc))
            }
            directory.append(area)
            directory.append(littleEndian(CRC32.checksum(area)))
        }

        var header = Data(repeating: 0, count: OBCGridCodec.headerLength)
        header.replaceSubrange(0..<4, with: Data("OBCG".utf8))
        write(&header, at: 4, littleEndian(UInt16(1)))
        write(&header, at: 6, littleEndian(UInt16(OBCGridCodec.headerLength)))
        write(&header, at: 8, littleEndian(UInt32(dataOffset + payloads.count)))
        header[12] = 1                                        // product id: provenance only
        header[13] = OBCGridCodec.tierMosaic                  // the one tier code the bakery writes
        write(&header, at: 14, littleEndian(
            spec.observed ? OBCGridCodec.flagObserved : OBCGridCodec.flagForecast))
        write(&header, at: 16, littleEndian(
            UInt64(bitPattern: Int64(spec.validAt.timeIntervalSince1970.rounded()))))
        write(&header, at: 24, littleEndian(
            UInt64(bitPattern: Int64(spec.referenceTime.timeIntervalSince1970.rounded()))))
        write(&header, at: 32, littleEndian(UInt32(bitPattern: spec.southMicrodegrees)))
        write(&header, at: 36, littleEndian(UInt32(bitPattern: spec.westMicrodegrees)))
        write(&header, at: 40, littleEndian(spec.cellMicrodegrees))
        write(&header, at: 44, littleEndian(spec.cellMicrodegrees))
        write(&header, at: 48, littleEndian(spec.width))
        write(&header, at: 52, littleEndian(spec.height))
        write(&header, at: 56, littleEndian(spec.cellSizeMetres))
        write(&header, at: 58, littleEndian(spec.tileEdge))
        write(&header, at: 60, littleEndian(spec.entriesPerPage))
        write(&header, at: 64, littleEndian(UInt32(OBCGridCodec.headerLength)))
        write(&header, at: 68, littleEndian(UInt32(dataOffset)))
        write(&header, at: 72, littleEndian(UInt32(payloads.count)))

        // Both CRCs are computed over the whole object with both fields zero, so the object CRC has
        // to be stamped before the header CRC — which then covers it as written.
        var object = header + directory + payloads
        let objectCRC = OBCGridCodec.objectCRC(object)
        write(&object, at: 76, littleEndian(objectCRC))
        let headerCRC = OBCGridCodec.headerCRC(object.prefix(OBCGridCodec.headerLength))
        write(&object, at: 80, littleEndian(headerCRC))
        return object
    }

    private static func littleEndian<T: FixedWidthInteger>(_ value: T) -> Data {
        withUnsafeBytes(of: value.littleEndian) { Data($0) }
    }

    private static func write(_ data: inout Data, at offset: Int, _ bytes: Data) {
        data.replaceSubrange(offset..<(offset + bytes.count), with: bytes)
    }
}
