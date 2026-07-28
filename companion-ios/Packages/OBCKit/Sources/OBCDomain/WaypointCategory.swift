import Foundation

/// What a waypoint *is*, in the device's own vocabulary: the six browsable POI
/// categories the map already uses (`OBCM_Spec.md` §7.4), reused verbatim for
/// waypoints so one icon language covers both sources on the device's "Up ahead"
/// list.
///
/// The raw values are the **stable wire ids** stored in an OBCR waypoint record's
/// category byte (`OBCR_Spec.md` §4). `0` is not a case: it means *generic*, which
/// this type models as `nil` — most hand-placed waypoints ("turn left here") map to
/// nothing, and generic is first-class, not a failure.
public enum WaypointCategory: UInt8, CaseIterable, Sendable {
    case water = 1
    case campsite = 2
    case accommodation = 3
    case resupply = 4
    case pharmacy = 5
    case bikeShop = 6

    /// The stored category byte (`0` = generic) for an optional category.
    public static func wireID(_ category: WaypointCategory?) -> UInt8 {
        category?.rawValue ?? 0
    }

    /// The category a stored byte names, or `nil` for generic — **including** any
    /// value outside `1...6`, which the spec says to render as generic rather than
    /// reject (a newer producer may know a category this build doesn't).
    public init?(wireID: UInt8) {
        self.init(rawValue: wireID)
    }

    /// Stable, device-facing label (matches the firmware's `PoiCategory::name`).
    public var label: String {
        switch self {
        case .water: return "Water"
        case .campsite: return "Campsite"
        case .accommodation: return "Lodging"
        case .resupply: return "Resupply"
        case .pharmacy: return "Pharmacy"
        case .bikeShop: return "Bike shop"
        }
    }
}
