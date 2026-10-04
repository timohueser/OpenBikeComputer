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
    static let entries: [String: Entry] = {
        guard let url = Bundle.module.url(forResource: "poi-kinds", withExtension: "json", subdirectory: "Map"),
              let data = try? Data(contentsOf: url) else { return [:] }
        return (try? JSONDecoder().decode([String: Entry].self, from: data)) ?? [:]
    }()
    static func kind(for value: String) -> PlannerPreviewPlace.Kind {
        if value == "cafe" { return .cafe }
        let searchKinds = ["campsite": "camp", "water_point": "water", "water_tap": "water", "hut": "hotel",
                           "bike_shop": "bike", "repair_station": "bike", "charging": "bike",
                           "summit": "peak", "pass": "peak", "bus_stop": "station"]
        let category = entries[value]?.category ?? searchKinds[value] ?? value
        return PlannerPreviewPlace.Kind(rawValue: category == "camp" ? "camping" : category) ?? .town
    }
    static func searchKinds(in categories: Set<String>) -> [String] {
        let kinds = ["hotel": ["lodging"], "camp": ["campsite"], "shelter": ["shelter"], "shop": ["resupply"],
                     "food": ["food"], "water": ["water"], "toilets": ["toilets"], "bike": ["bike"],
                     "pharmacy": ["pharmacy"], "station": ["train_station", "bus_stop", "ferry"],
                     "viewpoint": ["viewpoint"], "peak": ["summit", "pass"]]
        return categories.sorted().flatMap { kinds[$0] ?? [] }
    }
    nonisolated static func geoJSON(_ places: [PlannerPlace]) throws -> Data {
        let features: [[String: Any]] = places.map { place in
            let category = switch place.kind {
            case "campsite": "camp"
            case "water_point", "water_tap": "water"
            case "hut": "hotel"
            case "bike_shop", "repair_station", "charging": "bike"
            case "summit", "pass": "peak"
            case "bus_stop", "train_station", "ferry": "station"
            default: entries[place.kind]?.category ?? place.kind
            }
            return ["type": "Feature", "id": place.source,
                    "geometry": ["type": "Point", "coordinates": [place.lon, place.lat]],
                    "properties": ["kind": place.kind, "category": category, "name": place.name,
                                   "opening_hours": place.opening_hours ?? "", "website": place.website ?? "",
                                   "phone": place.phone ?? "", "description": place.description ?? ""]]
        }
        return try JSONSerialization.data(withJSONObject: ["type": "FeatureCollection", "features": features])
    }
    static func kinds(in categories: Set<String>) -> [String] {
        entries.filter { categories.contains($0.value.category) }.map(\.key).sorted()
    }
}
