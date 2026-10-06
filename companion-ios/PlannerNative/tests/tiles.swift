import Foundation

@main struct TileTests {
    struct Expected: Decodable { let z: Int; let x: Int; let y: Int; let text: String }
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let archive = try PMTilesArchive(root.appendingPathComponent("tiles.pmtiles"))
        let expected = try JSONDecoder().decode([Expected].self, from: Data(contentsOf: root.appendingPathComponent("expected.json")))
        for tile in expected {
            let actual = try archive.tile(z: tile.z, x: tile.x, y: tile.y)
            guard actual == Data(tile.text.utf8) else { throw URLError(.cannotDecodeContentData) }
        }
        guard try archive.tile(z: 9, x: 0, y: 0) == nil else { throw URLError(.cannotParseResponse) }
        var invalid = try Data(contentsOf: root.appendingPathComponent("tiles.pmtiles"))
        invalid.replaceSubrange(8..<16, with: repeatElement(UInt8(255), count: 8))
        let broken = root.appendingPathComponent("invalid.pmtiles")
        try invalid.write(to: broken)
        do { _ = try PMTilesArchive(broken); fatalError("Invalid archive was accepted") }
        catch let error as URLError { guard error.code == .cannotParseResponse else { throw error } }
        print("PMTiles: \(expected.count) tiles match the upstream writer; missing and malformed data handled")
    }
}
