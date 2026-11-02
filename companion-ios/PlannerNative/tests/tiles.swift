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
        func served(_ format: String, _ z: Int, _ x: Int, _ y: Int, mapZoom: Int?) throws -> String? {
            guard let file = offlinePack(root.appendingPathComponent(format), kind: "basemap", z: z, x: x, y: y, mapZoom: mapZoom) else { return nil }
            return try PMTilesArchive(file).tile(z: z, x: x, y: y).map { String(decoding: $0, as: UTF8.self) }
        }
        let offline: [(String, Int?, Int, Int, Int, String?)] = [
            ("format2", 11, 9, 267, 178, "tile 9/267/178"), ("format2", 11, 13, 4273, 2849, "tile 13/4273/2849"),
            ("format3", nil, 6, 33, 22, "tile 6/33/22"), ("format3", nil, 8, 133, 89, "tile 8/133/89"),
            ("format3", nil, 9, 267, 178, "tile 9/267/178"), ("format3", nil, 11, 1069, 713, "tile 11/1069/713"),
            ("format3", nil, 10, 535, 357, nil), ("format3", nil, 9, 268, 178, nil), ("format3", nil, 9, 264, 176, nil),
        ]
        for (format, mapZoom, z, x, y, text) in offline {
            guard try served(format, z, x, y, mapZoom: mapZoom) == text else { throw URLError(.fileDoesNotExist) }
        }
        print("PMTiles: \(expected.count) tiles match the upstream writer; missing and malformed data handled; "
            + "\(offline.count) offline tiles found in their packs")
    }
}
