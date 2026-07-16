import Foundation

/// A stored route's **retention level** — how long the device keeps a route after
/// its last use before auto-deleting it (epic #638). The wire enum (`u8`) the
/// phone sets via `setRouteRetention` (spec §4.4 cmd 6) and the device reports in
/// each `routeList` entry (spec §7.4). Deletion is device-only; the app's library
/// keeps the route forever and re-upload is one tap.
///
/// `expiry = last_used + retention`, computed device-side; the app never runs the
/// math — it displays the device's `expires_at` (``PlannedRouteRecord/deviceExpiresAt``)
/// and pushes the desired level here.
public enum Retention: UInt8, CaseIterable, Equatable, Sendable {
    case never = 0
    case oneDay = 1
    case oneWeek = 2
    case twoWeeks = 3
    case oneMonth = 4
    case twoMonths = 5

    /// The app's default retention for a **new** upload (epic #638: two weeks) —
    /// the level a route opts into when it's first put on the device and no
    /// explicit choice has been made. S7 wires the user-configurable setting;
    /// until then this is what an upload seeds behind the transport API.
    public static let appDefault: Retention = .twoWeeks

    /// Days after last use before the device deletes the route, or `nil` for
    /// ``never`` (no expiry). Display-independent (S7 formats it); the values
    /// mirror the epic's locked table — `1 month` = 30 days, `2 months` = 60.
    public var days: Int? {
        switch self {
        case .never: nil
        case .oneDay: 1
        case .oneWeek: 7
        case .twoWeeks: 14
        case .oneMonth: 30
        case .twoMonths: 60
        }
    }

    /// Decode a stored/wire retention byte, sanitising any **unknown** value to
    /// ``never`` — the firmware's safe-read posture (spec §4.4): a forward-compat
    /// byte must never surprise-delete a route, so it reads as "keep forever".
    public init(safeRawValue raw: UInt8) {
        self = Retention(rawValue: raw) ?? .never
    }
}
