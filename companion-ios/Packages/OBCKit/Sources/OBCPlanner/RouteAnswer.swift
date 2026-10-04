import Foundation
import OBCDomain

/// One route of a route answer, decoded from the integer deltas that `specs/route-api.md` specifies.
/// The companion does not read the surface and pushing runs.
struct RouteAnswer: Decodable {
    /// `start` and `end` are opaque road positions. A request can pin its first or last point to one.
    struct Leg: Decodable { let from_index: Int; let to_index: Int; let start: String; let end: String; let totals: Totals }
    struct Totals: Decodable { let distance_m: Double; let ascent_m: Double; let seconds: Double }
    let package: String
    let profile: String
    let coordinates: [Coordinate]
    let elevation: [Double?]
    let elapsed: [Double]
    let legs: [Leg]
    let totals: Totals

    private enum CodingKeys: String, CodingKey {
        case package, profile, coordinates_udeg, elevation_dm, elapsed_s, legs, totals
    }

    init(from decoder: any Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        package = try values.decode(String.self, forKey: .package)
        profile = try values.decode(String.self, forKey: .profile)
        legs = try values.decode([Leg].self, forKey: .legs)
        totals = try values.decode(Totals.self, forKey: .totals)
        let deltas = try values.decode([Int].self, forKey: .coordinates_udeg)
        guard deltas.count.isMultiple(of: 2) else {
            throw DecodingError.dataCorruptedError(forKey: .coordinates_udeg, in: values, debugDescription: "Unpaired coordinate")
        }
        coordinates = decodeCoordinates(deltas)
        var height = 0
        elevation = try values.decode([Int?].self, forKey: .elevation_dm).map { delta in
            delta.map { height &+= $0; return Double(height) / 10 }
        }
        var time = 0
        elapsed = try values.decode([Int].self, forKey: .elapsed_s).map { time &+= $0; return Double(time) }
    }
}

/// Coordinates from integer microdegree pairs, longitude first: the first pair absolute, each later pair the
/// difference from the one before. Wrapping sums cannot trap on corrupt data; the caller's range checks reject it.
func decodeCoordinates(_ udeg: [Int]) -> [Coordinate] {
    var longitude = 0, latitude = 0
    return stride(from: 0, to: udeg.count - 1, by: 2).map { index in
        longitude &+= udeg[index]
        latitude &+= udeg[index + 1]
        return Coordinate(latitude: Double(latitude) / 1e6, longitude: Double(longitude) / 1e6)
    }
}
