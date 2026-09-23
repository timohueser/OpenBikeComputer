import Foundation

/// The four fixed bike types of `specs/OBCR_Spec.md` §1.2. The raw value is the OBCR header byte
/// and the map's routing-profile index.
public enum BikeType: UInt8, CaseIterable, Sendable {
    case road = 0
    case gravel = 1
    case mtb = 2
    case touring = 3

    public var name: String {
        switch self {
        case .road: "Road"
        case .gravel: "Gravel"
        case .mtb: "MTB"
        case .touring: "Touring"
        }
    }

    /// Flat speed `v` in km/h and climb cost `k` in tenths of a second per metre of ascent.
    private var etaRow: (v: UInt64, k: UInt64) {
        switch self {
        case .road: (22, 16)
        case .gravel: (19, 19)
        case .mtb: (16, 23)
        case .touring: (17, 22)
        }
    }

    /// Whole seconds to ride `distanceMeters` while climbing `ascentMeters`:
    /// `floor((36·d + a·k·v) / (10·v))`, saturating at `UInt32.max`. Integer arithmetic, so the
    /// phone and the device give the same second.
    public func estimatedSeconds(distanceMeters: UInt32, ascentMeters: UInt32) -> UInt32 {
        let (v, k) = etaRow
        let seconds = (36 * UInt64(distanceMeters) + UInt64(ascentMeters) * k * v) / (10 * v)
        return UInt32(clamping: seconds)
    }

    /// The estimate for a summary's metre figures. They hold the OBCR header's whole metres for a
    /// phone-imported route, so truncation gives back the device's inputs.
    public func estimatedDuration(distanceMeters: Double, ascentMeters: Double) -> TimeInterval {
        func whole(_ meters: Double) -> UInt32 { UInt32(clamping: Int64(max(0, min(meters, 1e12)))) }
        return TimeInterval(estimatedSeconds(distanceMeters: whole(distanceMeters), ascentMeters: whole(ascentMeters)))
    }
}
