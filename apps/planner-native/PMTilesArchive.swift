import Foundation
import zlib

/// An immutable PMTiles v3 archive. Only requested directories and tiles are decoded.
final class PMTilesArchive {
    private struct Entry {
        var id: UInt64
        var offset: UInt64 = 0
        var length: UInt64 = 0
        var run: UInt64 = 0
    }
    private let file: Data
    private let internalCompression: UInt8
    private let tileCompression: UInt8
    private let leafOffset: UInt64
    private let leafLength: UInt64
    private let tileOffset: UInt64
    private let tileLength: UInt64
    private let root: [Entry]
    private var leaves: [(UInt64, [Entry])] = []
    private let lock = NSLock()

    init(_ url: URL) throws {
        let file = try Data(contentsOf: url, options: .mappedIfSafe)
        guard file.count >= 127, file.prefix(8) == Data([80, 77, 84, 105, 108, 101, 115, 3]),
              [1, 2].contains(file[97]), [1, 2].contains(file[98]) else { throw URLError(.cannotParseResponse) }
        func number(_ offset: Int) -> UInt64 { file.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: UInt64.self).littleEndian } }
        self.file = file
        internalCompression = file[97]; tileCompression = file[98]
        leafOffset = number(40); leafLength = number(48)
        tileOffset = number(56); tileLength = number(64)
        for (offset, length) in [(number(8), number(16)), (leafOffset, leafLength), (tileOffset, tileLength)] {
            guard offset <= file.count, length <= UInt64(file.count) - offset else { throw URLError(.cannotParseResponse) }
        }
        guard number(8) >= 127, number(8) + number(16) <= 16384 else { throw URLError(.cannotParseResponse) }
        root = try Self.directory(Self.slice(file, number(8), number(16)), compression: internalCompression)
    }

    func tile(z: Int, x: Int, y: Int) throws -> Data? {
        guard (0...22).contains(z), (0..<(1 << z)).contains(x), (0..<(1 << z)).contains(y) else { throw URLError(.badURL) }
        let wanted = Self.tileID(z: z, x: x, y: y)
        lock.lock(); defer { lock.unlock() }
        var directory = root
        for _ in 0..<4 {
            var low = 0, high = directory.count
            while low < high {
                let middle = (low + high) / 2
                if directory[middle].id <= wanted { low = middle + 1 } else { high = middle }
            }
            guard low > 0 else { return nil }
            let entry = directory[low - 1]
            if entry.run > 0 {
                guard wanted - entry.id < entry.run else { return nil }
                guard entry.offset <= tileLength, entry.length <= tileLength - entry.offset else { throw URLError(.cannotParseResponse) }
                let data = try Self.slice(file, tileOffset + entry.offset, entry.length)
                return tileCompression == 2 ? try Self.inflate(data) : data
            }
            guard entry.offset <= leafLength, entry.length <= leafLength - entry.offset else { throw URLError(.cannotParseResponse) }
            if let index = leaves.firstIndex(where: { $0.0 == entry.offset }) {
                let cached = leaves.remove(at: index); leaves.append(cached); directory = cached.1
            } else {
                directory = try Self.directory(Self.slice(file, leafOffset + entry.offset, entry.length), compression: internalCompression)
                leaves.append((entry.offset, directory))
                if leaves.count > 4 { leaves.removeFirst() }
            }
        }
        throw URLError(.cannotParseResponse)
    }
    private static func slice(_ data: Data, _ offset: UInt64, _ length: UInt64) throws -> Data {
        guard offset <= data.count, length <= UInt64(data.count) - offset, length <= 32 * 1024 * 1024 else {
            throw URLError(.dataLengthExceedsMaximum)
        }
        return data.subdata(in: Int(offset)..<Int(offset + length))
    }
    private static func directory(_ data: Data, compression: UInt8) throws -> [Entry] {
        let data = compression == 2 ? try inflate(data) : data
        var cursor = 0
        func integer() throws -> UInt64 {
            var value: UInt64 = 0
            for shift in stride(from: 0, to: 64, by: 7) {
                guard cursor < data.count else { throw URLError(.cannotParseResponse) }
                let byte = data[cursor]; cursor += 1
                guard shift < 63 || byte < 2 else { throw URLError(.cannotParseResponse) }
                value |= UInt64(byte & 127) << shift
                if byte & 128 == 0 { return value }
            }
            throw URLError(.cannotParseResponse)
        }
        func sum(_ a: UInt64, _ b: UInt64) throws -> UInt64 {
            let result = a.addingReportingOverflow(b)
            guard !result.overflow else { throw URLError(.cannotParseResponse) }
            return result.partialValue
        }
        let count = try integer()
        guard count > 0, count <= 1_000_000, count <= data.count / 4 else { throw URLError(.cannotParseResponse) }
        var entries: [Entry] = [], previous: UInt64 = 0
        entries.reserveCapacity(Int(count))
        for index in 0..<Int(count) {
            let delta = try integer()
            guard index == 0 || delta > 0 else { throw URLError(.cannotParseResponse) }
            previous = try sum(previous, delta)
            entries.append(Entry(id: previous))
        }
        for index in entries.indices { entries[index].run = try integer() }
        for index in entries.indices {
            entries[index].length = try integer()
            guard entries[index].length > 0 else { throw URLError(.cannotParseResponse) }
        }
        for index in entries.indices {
            let value = try integer()
            if value == 0 {
                guard index > 0 else { throw URLError(.cannotParseResponse) }
                entries[index].offset = try sum(entries[index - 1].offset, entries[index - 1].length)
            } else { entries[index].offset = value - 1 }
        }
        guard cursor == data.count else { throw URLError(.cannotParseResponse) }
        return entries
    }
    private static func tileID(z: Int, x: Int, y: Int) -> UInt64 {
        var result = ((1 << (z * 2)) - 1) / 3
        var x = x, y = y
        if z > 0 {
            for a in stride(from: z - 1, through: 0, by: -1) {
                let size = 1 << a, rx = size & x, ry = size & y
                result += ((3 * rx) ^ ry) << a
                if ry == 0 {
                    if rx != 0 { x = size - 1 - x; y = size - 1 - y }
                    swap(&x, &y)
                }
            }
        }
        return UInt64(result)
    }
    static func inflate(_ data: Data) throws -> Data {
        var stream = z_stream()
        guard inflateInit2_(&stream, 15 + 32, ZLIB_VERSION, Int32(MemoryLayout<z_stream>.size)) == Z_OK else { throw URLError(.cannotDecodeContentData) }
        defer { inflateEnd(&stream) }
        return try data.withUnsafeBytes { input in
            stream.next_in = UnsafeMutablePointer(mutating: input.bindMemory(to: Bytef.self).baseAddress)
            stream.avail_in = uInt(input.count)
            var result = Data(), buffer = [UInt8](repeating: 0, count: 64 * 1024)
            while true {
                let (status, count) = buffer.withUnsafeMutableBytes { output -> (Int32, Int) in
                    stream.next_out = output.bindMemory(to: Bytef.self).baseAddress; stream.avail_out = uInt(output.count)
                    let status = zlib.inflate(&stream, Z_NO_FLUSH)
                    return (status, output.count - Int(stream.avail_out))
                }
                guard result.count + count <= 32 * 1024 * 1024 else { throw URLError(.dataLengthExceedsMaximum) }
                result.append(contentsOf: buffer.prefix(count))
                if status == Z_STREAM_END { return result }
                guard status == Z_OK, count > 0 else { throw URLError(.cannotDecodeContentData) }
            }
        }
    }
}
