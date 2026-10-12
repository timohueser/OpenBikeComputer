import Foundation

/// Receiver for the frames that web/fountain.js sends; the frame layout and the block subsets are described there.
/// Decoding is online Gaussian elimination over GF(2): each frame is reduced against the rows
/// kept so far, and every frame that is not a combination of earlier ones becomes a new row.
struct FountainDecoder {
    enum Outcome: Equatable {
        case invalid, duplicate, redundant, useful
        case complete(Data)
        case corrupt
    }

    private struct Row {
        var coefficients: [UInt32]
        var data: [UInt8]
    }

    private(set) var crc: UInt32?
    private(set) var length = 0
    private(set) var blockCount = 0
    private(set) var rank = 0
    private(set) var framesSeen = 0
    private var blockSize = 0
    private var rows: [Row?] = []
    private var seen = Set<UInt32>()

    mutating func receive(_ text: String) -> Outcome {
        guard let bytes = base45Decode(text), bytes.count > 14 else { return .invalid }
        let crc = be32(bytes, 0), length = Int(be32(bytes, 4)), blockSize = Int(be16(bytes, 8)), seq = be32(bytes, 10)
        guard blockSize > 0, bytes.count == 14 + blockSize else { return .invalid }
        if crc != self.crc || length != self.length || blockSize != self.blockSize {
            self = FountainDecoder()
            self.crc = crc
            self.length = length
            self.blockSize = blockSize
            blockCount = max(1, (length + blockSize - 1) / blockSize)
            rows = Array(repeating: nil, count: blockCount)
        }
        guard seen.insert(seq).inserted else { return .duplicate }
        framesSeen += 1

        var row = Row(coefficients: fountainCoefficients(seq: seq, blockCount: blockCount, crc: crc), data: Array(bytes[14...]))
        while let pivot = lowestBit(row.coefficients) {
            guard let existing = rows[pivot] else {
                rows[pivot] = row
                rank += 1
                return rank == blockCount ? solve() : .useful
            }
            xor(&row, existing)
        }
        return .redundant
    }

    private mutating func solve() -> Outcome {
        for pivot in stride(from: blockCount - 1, through: 0, by: -1) {
            var row = rows[pivot]!
            for higher in pivot + 1 ..< blockCount where bit(row.coefficients, higher) {
                xor(&row, rows[higher]!)
            }
            rows[pivot] = row
        }
        let payload = Data(rows.flatMap { $0!.data }.prefix(length))
        return crc32(payload) == crc ? .complete(payload) : .corrupt
    }

    private func xor(_ row: inout Row, _ other: Row) {
        for i in row.coefficients.indices { row.coefficients[i] ^= other.coefficients[i] }
        row.data.withUnsafeMutableBufferPointer { a in
            other.data.withUnsafeBufferPointer { b in
                for i in a.indices { a[i] ^= b[i] }
            }
        }
    }
}

func fountainCoefficients(seq: UInt32, blockCount: Int, crc: UInt32) -> [UInt32] {
    var words = [UInt32](repeating: 0, count: (blockCount + 31) / 32)
    if Int(seq) < blockCount {
        words[Int(seq) >> 5] = 1 << (seq & 31)
        return words
    }
    let key = fmix32(seq ^ crc)
    for w in words.indices { words[w] = fmix32(key &+ UInt32(w)) }
    if blockCount % 32 != 0 { words[words.count - 1] &= (1 << UInt32(blockCount % 32)) - 1 }
    if words.allSatisfy({ $0 == 0 }) {
        let i = Int(seq) % blockCount
        words[i >> 5] = 1 << UInt32(i & 31)
    }
    return words
}

private func fmix32(_ x: UInt32) -> UInt32 {
    var h = x
    h ^= h >> 16
    h &*= 0x85EB_CA6B
    h ^= h >> 13
    h &*= 0xC2B2_AE35
    h ^= h >> 16
    return h
}

private func lowestBit(_ words: [UInt32]) -> Int? {
    for (w, word) in words.enumerated() where word != 0 { return w * 32 + word.trailingZeroBitCount }
    return nil
}

private func bit(_ words: [UInt32], _ i: Int) -> Bool { words[i >> 5] & (1 << UInt32(i & 31)) != 0 }
private func be32(_ b: [UInt8], _ o: Int) -> UInt32 { b[o ..< o + 4].reduce(0) { $0 << 8 | UInt32($1) } }
private func be16(_ b: [UInt8], _ o: Int) -> UInt16 { UInt16(b[o]) << 8 | UInt16(b[o + 1]) }

private let base45Alphabet = Array("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:".utf8)
private let base45Values: [UInt8: Int] = Dictionary(uniqueKeysWithValues: base45Alphabet.enumerated().map { ($1, $0) })

func base45Decode(_ text: String) -> [UInt8]? {
    let chars = Array(text.utf8)
    guard chars.count % 3 != 1 else { return nil }
    var out: [UInt8] = []
    out.reserveCapacity(chars.count / 3 * 2 + 1)
    var i = 0
    while i < chars.count {
        let group = chars[i ..< min(i + 3, chars.count)]
        var n = 0, factor = 1
        for c in group {
            guard let v = base45Values[c] else { return nil }
            n += v * factor
            factor *= 45
        }
        if group.count == 3 {
            guard n <= 0xFFFF else { return nil }
            out += [UInt8(n >> 8), UInt8(n & 0xFF)]
        } else {
            guard n <= 0xFF else { return nil }
            out.append(UInt8(n))
        }
        i += 3
    }
    return out
}

private let crcTable: [UInt32] = (0 ..< 256).map { n in
    (0 ..< 8).reduce(UInt32(n)) { c, _ in c & 1 != 0 ? 0xEDB8_8320 ^ (c >> 1) : c >> 1 }
}

func crc32(_ data: Data) -> UInt32 {
    ~data.reduce(~UInt32(0)) { crcTable[Int(($0 ^ UInt32($1)) & 0xFF)] ^ ($0 >> 8) }
}
