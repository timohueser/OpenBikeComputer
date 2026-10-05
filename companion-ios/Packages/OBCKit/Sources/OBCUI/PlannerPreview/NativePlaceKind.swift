import Foundation
import OBCPlanner

enum NativePlaceKind {
    static func source(for identifier: Any?) -> String? {
        guard let identifier else { return nil }
        let text = String(describing: identifier)
        if text.range(of: #"^[nwr][1-9][0-9]*$"#, options: .regularExpression) != nil { return text }
        guard let value = UInt64(text) else { return nil }
        let type = value >> 44, identity = value & ((1 << 44) - 1)
        guard (1...3).contains(type), identity > 0 else { return nil }
        return "\(["n", "w", "r"][Int(type) - 1])\(identity)"
    }
    struct Entry: Decodable { let category: String; let label: String }
    /// Basemap `pois` kinds (builder/app/src/lib/planner/poi-kinds.json).
    static let entries: [String: Entry] = resource("poi-kinds") ?? [:]
    /// The search query language (apps/planner-search/query/contract.json).
    struct Contract: Decodable {
        struct Kind: Decodable { let category: String? }
        let kinds: [String: Kind]
        let data: [String: String]
        let categories: [String: [String]]
    }
    static let contract: Contract? = resource("contract")
    private static func resource<T: Decodable>(_ name: String) -> T? {
        guard let url = Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Map"),
              let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }
    /// The map category of a search-data kind or a basemap kind.
    static func category(of kind: String) -> String {
        if let owner = contract?.data[kind], let category = contract?.kinds[owner]?.category { return category }
        return entries[kind]?.category ?? kind
    }
    static func kind(for value: String) -> PlannerPreviewPlace.Kind {
        if value == "cafe" { return .cafe }
        let name = category(of: value)
        return PlannerPreviewPlace.Kind(rawValue: name == "camp" ? "camping" : name) ?? .town
    }
    static func searchKinds(in categories: Set<String>) -> [String] {
        categories.sorted().flatMap { contract?.categories[$0] ?? [] }
    }
    nonisolated static func geoJSON(_ places: [PlannerPlace]) throws -> Data {
        let features: [[String: Any]] = places.map { place in
            ["type": "Feature", "id": place.source,
             "geometry": ["type": "Point", "coordinates": [place.lon, place.lat]],
             "properties": ["kind": place.kind, "category": category(of: place.kind), "name": place.name,
                            "opening_hours": place.opening_hours ?? "", "website": place.website ?? "",
                            "phone": place.phone ?? "", "description": place.description ?? ""]]
        }
        return try JSONSerialization.data(withJSONObject: ["type": "FeatureCollection", "features": features])
    }
    static func kinds(in categories: Set<String>) -> [String] {
        entries.filter { categories.contains($0.value.category) }.map(\.key).sorted()
    }
}
