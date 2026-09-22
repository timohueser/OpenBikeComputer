import Foundation

/// A digest-pinned object: exact length, lowercase SHA-256, and a URL that carries that digest
/// (OBCC §9).
struct Pin: Decodable, Sendable {
    let bytes: Int
    let sha256: String
    let url: String
}

/// The part of the catalog root (`specs/OBCC_Spec.md` §3) the router reads. The root text itself
/// goes to the assembler, which reads the schema, a skin and the terrain lattice from it.
struct CatalogRoot: Decodable, Sendable {
    struct Schema: Decodable, Sendable {
        let bands: [Band]
    }

    struct Band: Decodable, Sendable {
        let id: String
        let cellLog2: Int
        let role: String
    }

    struct IndexRef: Decodable, Sendable {
        let band: String
        let bytes: Int
        let sha256: String
        let url: String

        var pin: Pin { Pin(bytes: bytes, sha256: sha256, url: url) }
    }

    struct Terrain: Decodable, Sendable {
        let cellLog2: Int
        let cellIndex: Pin
    }

    let schemaVersion: Int
    let schema: Schema
    let cellIndex: [IndexRef]
    let terrain: Terrain?

    /// The one band that carries the routing graph.
    var core: (band: Band, index: Pin)? {
        guard let band = schema.bands.first(where: { $0.role == "core" }),
              let ref = cellIndex.first(where: { $0.band == band.id }) else { return nil }
        return (band, ref.pin)
    }
}

/// A pinned cell index: a band's (§8) or the terrain's (§13.1). The two share this shape.
struct CellIndex: Decodable, Sendable {
    struct Entry: Decodable, Sendable {
        let id: String
        let bytes: Int
        let sha256: String
        let url: String
        let partial: Bool?

        var pin: Pin { Pin(bytes: bytes, sha256: sha256, url: url) }
    }

    struct Run: Decodable, Sendable {
        let start: String
        let end: String
    }

    let cells: [Entry]
    let knownEmpty: [Run]

    enum Answer {
        case artifact(Entry)
        case knownEmpty
        /// The catalog does not cover the square: ground the router cannot reach.
        case hole
    }

    func lookup(_ cell: CellID) -> Answer {
        if let entry = cells.first(where: { CellID($0.id) == cell }) { return .artifact(entry) }
        let empty = knownEmpty.contains { run in
            guard let start = CellID(run.start), let end = CellID(run.end) else { return false }
            return start.log2 == cell.log2 && start.i == cell.i && start.j <= cell.j && cell.j <= end.j
        }
        return empty ? .knownEmpty : .hole
    }
}

/// Catalog documents spell their keys in snake case.
func decodeCatalog<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return try decoder.decode(type, from: data)
}
