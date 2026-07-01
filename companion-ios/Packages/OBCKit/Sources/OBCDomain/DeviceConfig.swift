import Foundation

/// The device's writable configuration — the semantic view of the OBC Control
/// `Config` characteristic / `config_blob`. Read with `readConfig()`, written
/// with `writeConfig(_:)`.
///
/// **Delta 1 — device name lives in `Config`.** Renaming the device (H3) is a
/// `writeConfig` with a changed `name`; there is no separate rename command. This
/// is a hard requirement on the wire contract — see `OBCProtocol.md` → *Delta 1*.
///
/// **B1 finalization:** carries the contract-mandated `name` plus the display
/// `units` the Settings screen (B8/G) edits. The rest of the config blob (display
/// prefs, sensor pairing, …) is B8's / firmware `S0`'s to grow — kept minimal here
/// so the config codec has a real, round-trippable shape without inventing fields.
public struct DeviceConfig: Equatable, Sendable {
    /// Unit system the device displays. Editable from Settings (G).
    public enum Units: UInt8, Equatable, Sendable, CaseIterable {
        case metric = 0
        case imperial = 1
    }

    /// User-facing device name. Writing this renames the device (H3).
    public var name: String
    /// Display unit system.
    public var units: Units

    public init(name: String, units: Units = .metric) {
        self.name = name
        self.units = units
    }
}
